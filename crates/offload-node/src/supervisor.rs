//! Where a run actually happens: workspace, then agent, then event pump.
//!
//! This drives `offload_core::Run` — the real state machine, with real epoch fencing —
//! rather than a simpler local status enum. On a single node this node is trivially its
//! own arbiter, so the fencing is not yet load-bearing. Using it anyway means phase 4 is
//! wiring rather than rewriting, and it puts the phase 0 model under real usage now,
//! which is when design errors are cheap to find.

use crate::api::{LogEvent, LogKind, RunSummary};
use crate::config::Config;
use offload_agent::claude::ClaudeCode;
use offload_agent::{transcript, AgentEvent, Resume, SessionId, SpawnRequest};
use offload_core::{
    AgentKind, AgentWork, Capacity, Checkpoint, Constraint, Epoch, GivenUp, Millis, NodeId,
    Occupancy, PermissionMode, Prospect, Restartability, Run, RunId, RunSpec, RunState,
    ToolAllowlist, Waiting, Work, WorkspaceSpec,
};
use offload_store::Store;
use offload_workspace::{Removal, RepoSource, Workspace, WorkspaceManager};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

/// The one lease duration, from the type it belongs to.
///
/// It used to be an hour here and a minute in the bid round — set when this node was the only
/// place a run could be and nobody could take one away, which stopped being true in phase 4.
/// A run does not hold differently depending on how it arrived.
use offload_core::LEASE as LEASE_TTL;

/// How long a peer that asked for a run's log still counts as watching it.
///
/// A remote follower polls about once a second, so this is a few missed polls — long enough that
/// a slow network is not mistaken for somebody walking away, short enough that walking away is
/// noticed before the decision that reads it (whether a failed run resumes itself) is taken.
const WATCHER_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a run waits for a person to answer a permission question (ADR-0017).
///
/// Minutes rather than hours, and the reason is the mid-turn rule: the agent is blocked inside a
/// tool call, so nothing can be checkpointed (ADR-0004), nothing can be drained, and the lease is
/// being renewed for a machine that is doing nothing. Five minutes is long enough for somebody
/// holding a phone and short enough that a run cannot be parked all night by a question. A queued
/// *notification* is the opposite case and rightly waits indefinitely (ADR-0010) — it is durable,
/// and a blocked tool call is not.
const DEFAULT_ASK_PATIENCE: Millis = Millis(300_000);

/// The least patience a question ever gets, whatever the deadline says.
///
/// A run that is already past its deadline still gets this: a missed deadline decides when we
/// give up, not what we are allowed to do (ADR-0013), and answering "denied" the instant a
/// question is asked would make the feature indistinguishable from not having it.
const MIN_ASK_PATIENCE: Millis = Millis(30_000);

/// Extra time the agent's own hook timeout gets over this node's patience.
///
/// The hook being killed and the daemon giving up mean the same thing, so the only requirement is
/// that the daemon decides first; otherwise the answer arrives at a process that is already dead.
const ASK_HOOK_MARGIN: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a run stopped by its turn limit waits for the agent to write the turn it just
/// finished, before it is stopped and captured. See [`Supervisor::await_transcript`].
///
/// Three seconds because the flush is sub-second when it happens at all — the runs that caught up
/// had done so by the time a 0.5s capture attempt reached them — and because this is time added
/// to the end of a run somebody asked to be *stopped*. Long enough to cover a loaded machine,
/// short enough that nobody watching wonders whether the limit worked.
const TRANSCRIPT_FLUSH_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// How often [`Supervisor::await_transcript`] re-reads while it waits.
const TRANSCRIPT_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// The subcommand this daemon runs itself under to serve as a permission hook.
pub const ASK_HOOK_ARG: &str = "ask-hook";

/// Environment the agent (and therefore its hook) is given, so a question can say who is asking.
pub const ASK_RUN_ENV: &str = "OFFLOAD_ASK_RUN";
pub const ASK_EPOCH_ENV: &str = "OFFLOAD_ASK_EPOCH";
pub const ASK_SOCKET_ENV: &str = "OFFLOAD_ASK_SOCKET";

/// The tools worth stopping a run to ask a person about, **under the mode it is running in**.
///
/// Short on purpose, and this is the list to argue with rather than to extend quietly. A
/// `PreToolUse` hook fires for *every* tool call — verified, not assumed — and the agent does not
/// tell the hook whether permission was actually needed. So a longer list does not mean better
/// coverage; it means asking a person about reads the agent would have allowed by itself, which is
/// the harness second-guessing the agent (ADR-0004).
///
/// Which is why it is a function of the mode rather than a constant. The mode *is* what decides
/// whether the agent gates something, so a list that ignored it would be wrong in both directions
/// at once, and both were once true here:
///
/// * Under `AcceptEdits` and `Full` the agent allows edits by itself. Matching `Write` there
///   would stop a run to ask about something nobody was ever going to be asked about — the
///   ADR-0004 line, crossed from the noisy side.
/// * Under `Ask` the agent gates *every* edit. Not matching `Write` there is the same line
///   crossed from the silent side, and worse: the run can be asked about a command and is
///   quietly denied every file it writes. That asymmetry is exactly why ADR-0008's refusal of
///   `Ask` stood until this list learned about the mode.
///
/// Measured on `claude 2.1.238`, because ADR-0017's rule is that this is measured: under
/// `--permission-mode manual` a `Write` is denied headless with no hook and carries a
/// `permission_denial`; with a `Write` matcher the hook fires with the call's own `tool_use_id`,
/// an `allow` writes the file, and the result reports no denials.
#[must_use]
pub fn ask_tools(mode: PermissionMode) -> &'static [&'static str] {
    match mode {
        // Commands and fetches: where the risk is, and where a headless denial costs a run its
        // work. Edits are the agent's own business in these modes.
        PermissionMode::AcceptEdits | PermissionMode::Full => &["Bash", "WebFetch"],
        // Everything the agent gates under `manual`. `Read` is deliberately absent: the agent
        // allows it, so a matcher would only buy questions.
        PermissionMode::Ask => &[
            "Bash",
            "WebFetch",
            "Write",
            "Edit",
            "MultiEdit",
            "NotebookEdit",
        ],
    }
}

/// A question in flight: where its answer will arrive, and how long there is.
#[derive(Debug)]
pub struct AskWait {
    pub answer: tokio::sync::oneshot::Receiver<crate::asks::Verdict>,
    pub patience: Millis,
}

/// Why an agent's question is not going to be put to a person.
///
/// Each variant is a case where asking would be *wrong* rather than merely unnecessary, and each
/// one ends the same way for the agent: nothing is granted and its own rules decide, which is
/// what happens on every run in the fleet today.
#[derive(Debug, thiserror::Error)]
pub enum NoAsk {
    #[error("run {0} is not in this node's registry")]
    Unknown(String),
    #[error("this node does not hold that run at that epoch (it is at e{epoch})")]
    NotOurs { epoch: u64 },
    #[error("that run was not submitted with --ask")]
    NotAsking,
    #[error("an existing grant already covers this, so the agent's own rules decide")]
    AlreadyGranted,
    #[error("there is no route to a person and nobody is watching this run")]
    NobodyToAsk,
    /// The run has asked as many questions as it was given.
    ///
    /// Not a denial, and the wording says so where somebody will read it: what happens to this
    /// tool call is the agent's own rules, exactly as if the run had never been able to ask.
    #[error(
        "this run has already asked its {asked} question(s); the rest is the agent's own rules"
    )]
    BudgetSpent { asked: u32 },
    #[error(transparent)]
    Store(#[from] offload_store::StoreError),
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error(
        "permission mode `ask` needs someone to answer the prompt, and this run has nobody. \
             Add --ask, which is how a headless run reaches a person (ADR-0017) — or use \
             --permission accept-edits (the default), or --permission full to bypass gating \
             entirely. See docs/adr/0008-headless-permissions.md"
    )]
    AskIsUnanswerable,
    /// No room here, in whichever of the two senses (ADR-0013): the owner's flat run ceiling,
    /// or the share budget a heavy run can spend on its own.
    #[error("node cannot take it: {}", describe_refusal(_0))]
    NoRoom(offload_core::Refusal),
    #[error("workspace: {0}")]
    Workspace(#[from] offload_workspace::WorkspaceError),
    #[error("agent: {0}")]
    Agent(String),
    /// A node named in `--prefer` or `--require` could not be identified (ADR-0063 §3).
    #[error("{0}")]
    Unresolved(#[from] offload_core::Unresolved),
    /// A spec that contradicts itself, refused before anything is decided about it.
    #[error("{0}")]
    Spec(#[from] offload_core::SpecProblem),
    /// A task could not be started. Its own variant rather than [`SubmitError::Agent`], which
    /// prefixed the first walk's refusal with the word "agent" for a run that has none.
    #[error("task: {0}")]
    Task(String),
    #[error("no run matching `{0}`")]
    NoSuchRun(String),
    /// This node has no way to run that kind of work (ADR-0019, phase 8).
    ///
    /// The `Work` split landed before the tier that executes a task, so every path below that
    /// spawns, resumes or prepares a workspace asks for the agent half and says this when there
    /// is none — rather than reading a field a task legitimately does not have. Unreachable
    /// today, because nothing constructs a `Work::Task` outside tests; present because
    /// *unreachable* and *silently mishandled* look identical at the call site.
    #[error("this node cannot host {kind} work: it has no {kind} runner (ADR-0019)")]
    UnsupportedWork { kind: &'static str },
    #[error("allowlist: {0}")]
    Allowlist(#[from] offload_core::PatternError),
    #[error("state store: {0}")]
    Store(#[from] offload_store::StoreError),
    /// A store error that is about what the **operator typed**, not about the store.
    ///
    /// `#[error(transparent)]` so it arrives without the `state store:` prefix: that prefix says
    /// *the database is where to look*, and for a missing argument it names the wrong thing
    /// entirely. Not a second sentence — the same one, unwrapped.
    #[error(transparent)]
    Needle(offload_store::StoreError),
    /// An operator asked for something this run is not in a position to do. Carries the
    /// action as well as the reason: "cannot be resumed: it is not running" reads as
    /// nonsense, and the two callers refuse for genuinely different reasons.
    #[error("run {run} cannot be {action}: {reason}")]
    Refused {
        run: String,
        action: &'static str,
        reason: String,
    },
    /// The run ended before this node could hold it, and no later tick will change that.
    ///
    /// Its own variant for the same reason [`SubmitError::LostTheRun`] is: the *caller* has to
    /// tell a refusal that will lift from one that never will. `start_held_runs` logged every
    /// failure under "could not start it yet", which is a promise the next tick will try again —
    /// and a terminal run holds no lease, so it never appears in `held_but_not_started` again.
    /// The sentence a person reads is deliberately unchanged from the `Refused` it replaced.
    #[error("run {run} cannot be {action}: it was {state} while it waited to start")]
    Ended {
        run: String,
        action: &'static str,
        state: &'static str,
    },
    /// The fence refused this leg: the run has moved on, and nothing about it is ours to say.
    ///
    /// Its own variant because the *caller* has to tell it from every other failure. `launch`
    /// treats an error from `drive` as "this run failed" and marks it so — through `abandon`,
    /// which is unfenced on purpose, because a run whose workspace could not be built has no
    /// holder to fence against. For this one error that is exactly backwards: the row by then is
    /// the **new holder's**, at the new holder's epoch, so a terminal state written into it beats
    /// the live record everywhere and `note_failure` makes the run a candidate for auto-resume.
    /// Somebody else is running it, the fleet says it failed, and recovery is entitled to start a
    /// second agent — double execution, reached through the fence firing.
    #[error("run {run} moved on: {reason}")]
    LostTheRun { run: String, reason: String },
    /// An agent for this run is **already going on this node**, so this leg is not starting a
    /// second one.
    ///
    /// Its own variant for the reason [`SubmitError::Ended`] and [`SubmitError::LostTheRun`] are:
    /// the caller has to tell it from a start that failed. Nothing is wrong with the run — it is
    /// running — so this is neither a failure to write down nor a refusal that will lift, and the
    /// leg that gets it must do *nothing*, least of all conclude the run.
    #[error("run {run} already has an agent going here")]
    AlreadyGoing { run: String },
}

/// What [`Supervisor::push_last_copies`] managed on the way out.
///
/// Four numbers rather than one, because a drain's whole job is to say what it could not do:
/// `refused` is a fleet that would not take a copy, `unattempted` is this node running out of
/// time, and they need opposite things from whoever reads them. `already` is the ordinary case
/// and is here so that "nothing was pushed" can be told from "nothing needed pushing".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LastCopies {
    /// Copies made here and now that would not otherwise have existed.
    pub pushed: u32,
    /// Nobody would take a copy. The run is as stranded as it was, and says so.
    pub refused: u32,
    /// The budget ran out first. Distinct from `refused`: the fleet was willing.
    pub unattempted: u32,
    /// Somebody else already held it, which is what the common case looks like.
    pub already: u32,
}

impl LastCopies {
    /// Was anything at risk at all — anything to say to whoever typed `offload drain`.
    #[must_use]
    pub fn worth_saying(&self) -> bool {
        self.pushed > 0 || self.refused > 0 || self.unattempted > 0
    }
}

/// A failed run this node might pick back up, and what it knows about it.
#[derive(Debug, Clone, Copy)]
struct Recovering {
    /// Whether anybody was watching *when it failed* — and `None` when this node cannot say,
    /// which is a real answer rather than a missing one: a daemon that restarted while a run was
    /// active never saw the stream that a client had open, and every such client was
    /// disconnected by the restart anyway.
    attendance: Option<offload_core::Attendance>,
    /// How many times this node has picked it up since. Counted here rather than taken from
    /// `Run::attempts`, which counts every assignment: a run that migrated three times and then
    /// crashed once has not been retried three times, and charging its travels against its
    /// retries would leave the 02:00 case with none.
    resumes: u32,
    /// The decision, once it has been taken to leave the run for a person.
    ///
    /// The entry used to be *removed* at that point, on the sound grounds that the reason should
    /// be said once. What that also removed was the only record of it: `offload explain` had
    /// nothing to print, and its `attendance` line then sampled the stream **now** and reported
    /// `unattended — nobody is streaming it` about a run this node had left alone precisely
    /// because somebody *was* watching. Measured on one daemon, one run, one second apart.
    ///
    /// So it is kept and the tick skips it, which says it once and remembers it. `None` means no
    /// decision yet — either a retry is pending, or the backoff has not elapsed.
    decided: Option<offload_core::Escalation>,
}

/// What has to stay in memory because it cannot be written down.
///
/// Everything durable — the run, its events, its stats — lives in the store now. What is
/// left is a broadcast channel and a cancel handle: process-scoped things that are
/// meaningless to a restarted daemon, and whose absence after a restart is itself the
/// correct answer (you cannot cancel a run whose agent process died with the daemon).
///
/// **One entry, three facts, three lifetimes — and the entry's own presence is none of them.**
/// This is the ADR-0005 question asked of a node-local map: each field is a different claim,
/// ending at a different moment, and a reader that asks the map instead of the field gets the
/// longest-lived answer to a question it did not ask.
///
/// | The fact | The field | True from | Until |
/// | --- | --- | --- | --- |
/// | an agent for this run is going *here, now* | `cancel` | [`Supervisor::register`] | [`Supervisor::release`], one step before the terminal write |
/// | this leg may still append to the run's log | `live` | `register` | [`Supervisor::close_stream`], at the end of `launch` |
/// | this node ran a leg of this run, numbered `epoch` | `epoch` | `register` | the run's **row** goes ([`Supervisor::forget_legs`]) |
///
/// The third is the only one that outlives the leg, and it has to: [`Supervisor::describes`]
/// asks whether a finished run ended *here*, and for a run with no holder this leg's number is
/// the only thing that can answer. Its owner is this node and nothing else — a leg is a fact
/// about a machine's own process, so it is never gossiped and there is nothing to arbitrate.
/// It stops being true when the run does, which is why `forget_legs` hangs off the pass that
/// deletes rows rather than off anything about the agent. Losing it early is safe and losing it
/// is *normal*: a restart empties this map, and every reader of the epoch has the restart's
/// answer already.
///
/// Ask the field. Presence answers all three at once, and for as long as the longest of them.
#[derive(Debug)]
struct LiveRun {
    epoch: Epoch,
    /// The run's live event stream, **`None` once this leg has stopped writing to it.**
    ///
    /// The entry outlives the run on purpose — `Supervisor::describes` asks whether a finished
    /// run ended *here*, and the only thing that can answer is this leg's epoch, which is what is
    /// left when this field goes. What must not outlive it is the channel: `broadcast::channel`
    /// allocates its ring up front, and measured on this machine that is **~37 KB per run**,
    /// retained for the daemon's lifetime whether or not anybody ever followed the run. ADR-0020
    /// is what makes that a leak rather than a rounding error — a trigger firing every three
    /// seconds is 1,200 runs an hour, and this daemon is meant to run on a phone.
    ///
    /// Closed by [`Supervisor::close_stream`] at the end of `launch`, which is the one place that
    /// knows every append for this leg has happened.
    live: Option<broadcast::Sender<LogEvent>>,
    /// Carries *why*, because stopping the agent here and ending the run are different facts
    /// (see [`Halt`]). A cancel also names the node it was typed at, so the run's log records
    /// where the decision came from — the same reason `ClusterMessage::Answer` carries a `by`.
    cancel: Option<mpsc::Sender<Halt>>,
    /// Somebody asked for a checkpoint, and **who** — honoured at the next turn boundary,
    /// because that is the only point where the worktree and the transcript agree (ADR-0004).
    ///
    /// A `bool` until ADR-0042, which is where the two facts with one spelling begin: `offload
    /// checkpoint` and a drain arm this same flag, and the run each of them leaves behind is
    /// `Pending` with a checkpoint beside it. Nothing downstream could tell them apart, because
    /// the only moment that knows is this one — several minutes and one turn before the release
    /// it decides the meaning of. So the answer travels with the request instead of being
    /// guessed at the other end.
    checkpoint_requested: Option<GivenUp>,
}

/// A continuation's refusal, in the one shape every door of it uses.
fn continuation_refused(parent: RunId, reason: String) -> SubmitError {
    SubmitError::Refused {
        run: parent.short(),
        action: "continued",
        reason,
    }
}

/// The parent's agent work, or the refusal for a task — which has no conversation and no
/// workspace to continue from (ADR-0064 §1). Asked on both sides of a forwarded continuation,
/// because both read the spec.
fn continued_work(parent: &Run) -> Result<&AgentWork, SubmitError> {
    parent.spec.agent().ok_or_else(|| {
        continuation_refused(
            parent.id,
            "it is a task: there is no conversation and no workspace to continue from".to_string(),
        )
    })
}

/// A checkpoint every blob of which is on **this** machine.
///
/// The type exists because the rule had two doors and only one of them was guarded.
/// `start_run` fetched what a checkpoint names before starting an agent from it; `resume` —
/// which is both `offload resume` and the recovery tick — built the same `Start::Resume` and
/// went straight to `launch`, so a run resumed on a node that had never been replicated to
/// died inside the restore with `blob … is not stored here`, having fetched nothing and
/// logged nothing about it. A comment saying "call `ensure_blobs` first" is a hope; a type
/// nobody can build without asking is the door.
///
/// [`Restorable::check`] is the only constructor and it cannot be satisfied without an answer
/// for every blob, so `Start::Resume` cannot be spelled without having asked.
/// A handoff continuation's prompt: what to do now, then one fixed heading carrying what came
/// before (ADR-0064 §3). **Offload writes none of it but the frame** — the two quoted strings are
/// the parent's own prompt and its own closing words, moved verbatim.
fn handoff_prompt(
    prompt: &str,
    parent: RunId,
    parent_prompt: &str,
    closing: Option<&str>,
    transcript: bool,
) -> String {
    let quote = |text: &str| {
        text.lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut out = format!(
        "{prompt}\n\n---\n\n## Continuing an earlier run\n\nThis run continues run {parent}. Its \
         work is already in this workspace — its commits on this branch, and whatever it left \
         uncommitted — so `git log` and `git diff` say what it did more exactly than anything \
         below.\n\nThe earlier run was asked:\n\n{}\n\n",
        quote(parent_prompt)
    );
    match closing {
        Some(closing) => out.push_str(&format!("Its own closing words:\n\n{}\n\n", quote(closing))),
        None => out.push_str("It ended with no closing message.\n\n"),
    }
    if transcript {
        out.push_str(&format!(
            "Its full transcript is at `{}`, as the agent wrote it. Consult it when the above is \
             not enough; there is no need to read it first. It is JSON Lines, one event per \
             line, and some lines are far too long to read whole — search it (for \
             `tool_result`, say) rather than opening it.\n",
            offload_core::PARENT_TRANSCRIPT
        ));
    } else {
        out.push_str("Its transcript could not be found, so the workspace is all there is.\n");
    }
    out
}

mod restorable {
    use offload_core::{BlobHash, Checkpoint};

    /// Which of this checkpoint's blobs are not here, asked without consuming it.
    pub fn missing(checkpoint: &Checkpoint, here: impl Fn(BlobHash) -> bool) -> Vec<BlobHash> {
        checkpoint
            .blobs()
            .into_iter()
            .filter(|hash| !here(*hash))
            .collect()
    }

    /// Boxed because a `Checkpoint` dwarfs the other `Start` variant, and that enum is passed
    /// by value into every run.
    #[derive(Debug)]
    pub struct Restorable(Box<Checkpoint>);

    impl Restorable {
        /// The only way to make one, and it asks about every blob the checkpoint names.
        ///
        /// Returns the ones that are still absent, which is what the caller has to say out
        /// loud — the count is the whole of what distinguishes "nobody could supply it" from
        /// "the fetch said it worked".
        pub fn check(
            checkpoint: Box<Checkpoint>,
            here: impl Fn(BlobHash) -> bool,
        ) -> Result<Self, Vec<BlobHash>> {
            let absent = missing(&checkpoint, here);
            if absent.is_empty() {
                Ok(Self(checkpoint))
            } else {
                Err(absent)
            }
        }
    }

    impl std::ops::Deref for Restorable {
        type Target = Checkpoint;
        fn deref(&self) -> &Checkpoint {
            &self.0
        }
    }
}

use restorable::Restorable;

/// How a run's agent starts: from its prompt, or from where it left off.
#[derive(Debug)]
enum Start {
    Fresh,
    Resume {
        checkpoint: Restorable,
        session: SessionId,
        prompt: String,
    },
    /// A continuation's first start (ADR-0064): the parent's work restored as the workspace, and
    /// the agent started fresh on the spec's prompt — or, in `Session` mode, the parent's
    /// conversation forked. Never a resume of *this* run, which has no conversation yet.
    Continue {
        checkpoint: Restorable,
        parent: offload_core::Continuation,
    },
}

/// How a resumed run's workspace came back.
///
/// The account is for the run's log, and `adopted` is a decision the transcript has to make
/// the same way: both live at the path this node derives from the worktree, so a checkout
/// that turned out to be from an earlier leg means the transcript beside it is too.
#[derive(Debug, Clone)]
struct Restored {
    how: String,
    adopted: bool,
}

/// Why the event pump stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stop {
    /// The agent's stream closed on its own.
    Finished,
    /// Something asked the agent here to stop. **Not the same as the run being over** — see
    /// [`Halt`].
    Halted(Halt),
    /// Captured at a turn boundary and handed back to the pool.
    Checkpointed,
    /// The run reached the turn limit its submitter gave it (`RunSpec::max_turns`).
    ///
    /// Its own variant rather than a `Checkpointed` with a flag, because the two are opposite
    /// decisions about the same captured state: a checkpoint hands the run *back to the pool*,
    /// and handing this one back is the one thing that must not happen — the next node to bid
    /// would pick it up and carry on past the limit, one leg at a time, which is the cap
    /// enforced everywhere except where it counts.
    LimitReached { limit: u32 },
}

/// Why an agent on this node is being asked to stop, which is not the same question as what
/// became of the run.
///
/// The two used to be one, and the collapse was a bug of exactly the shape `CLAUDE.md` warns
/// about with log kinds: the *only* way to stop an agent here was the cancel channel, and
/// whatever came down it was recorded as `Cancelled`. So a node that had lost a run — reassigned
/// under a higher epoch, or granted to somebody else at the same one — stopped its agent, which
/// is right, and then wrote a terminal state into the record it had *just* overwritten with the
/// new holder's copy. Unfenced, at the new holder's epoch, and terminal, so it beat the live
/// record everywhere except on the new holder itself (whose `absorb` refuses a peer's word about
/// a run it holds). The fleet said cancelled, the agent kept working, and the operator was told
/// their run had stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Halt {
    /// An operator cancelled the run, from the node named. The run is over, and this node is
    /// entitled to say so — it is the holder, which is what `cancel_run` checked before
    /// signalling.
    Cancelled { by: String },
    /// This node lost the run. The agent must stop; the record belongs to whoever holds it now,
    /// and nothing here is written down.
    Superseded,
    /// This node has been revoked from its fleet, so it may not be running anything (ADR-0044).
    ///
    /// Deliberately **not** [`Halt::Superseded`], though the agent stops the same way. Superseded
    /// writes nothing because somebody else's record has already arrived and is the truth; a
    /// revoked node has heard from nobody and will hear from nobody, so nothing has arrived and
    /// leaving the row `Running` behind a process that is gone is the lie `fail_if_unfinished`
    /// exists to stop.
    ///
    /// Not [`Halt::Cancelled`] either, for `Stop::LimitReached`'s reason: `by` names the node an
    /// operator typed `offload cancel` at, and there is no such node here.
    Revoked,
}

#[derive(Clone)]
pub struct Supervisor {
    config: Arc<Config>,
    node_id: NodeId,
    workspaces: WorkspaceManager,
    agent: ClaudeCode,
    /// Where the agent keeps its transcripts. Held here as well as
    /// inside the adapter because checkpoint and resume address transcripts by
    /// `(home, cwd, session)`, and after a `--fork-session` resume the session is one the
    /// agent chose rather than the one the handle was built with.
    home: PathBuf,
    store: Store,
    live: Arc<Mutex<BTreeMap<RunId, LiveRun>>>,
    /// This node has been drained and is not to take work again.
    ///
    /// `Mesh::drain`'s own doc comment has said "and stop accepting new ones" since it was
    /// written and nothing implemented it: it handed its runs over and returned, so a laptop
    /// about to be closed went on bidding and winning — measured, one second after `offload
    /// drain` reported "nothing to hand over", and the run it had just moved to the desktop
    /// could come straight back.
    ///
    /// Here rather than on the mesh because both halves of the answer already hold a
    /// supervisor: the bid and the grant (`NodeHost`), and the status somebody reads when work
    /// stops landing. One-way and in memory on purpose — a drain is a departure, so the way back
    /// is starting the daemon again, and a flag that expired would be machinery for a state that
    /// lasts as long as somebody's walk to the door.
    draining: Arc<std::sync::atomic::AtomicBool>,
    /// This node has been revoked from its fleet and must not be running anything (ADR-0044).
    ///
    /// Beside `draining` rather than folded into it, though both are one-way latches that stop
    /// this node taking work. They are two facts and an operator meets the difference: a drain is
    /// something somebody typed here and `offload status` tells them to restart the daemon to
    /// undo it, which for a revocation is advice that cannot work. And they differ where it
    /// counts — a drain hands its runs to the fleet, and a revoked node has no fleet to hand
    /// anything to.
    revoked: Arc<std::sync::atomic::AtomicBool>,
    /// Finished runs this node has not yet had a gossip exchange since (see
    /// [`Self::runs_to_publish`]). In memory: a restart republishes the last day's once.
    untold: Arc<Mutex<Untold>>,
    /// What this node saw when a run failed, and what it has done about it since.
    ///
    /// **In memory on purpose.** Attendance is an observation, not a field on the run: it is
    /// sampled at the instant the run is written `Failed` (see `offload_core::recovery` for why
    /// not later), and it belongs to the node that was serving the stream. A daemon that
    /// restarts forgets, and forgetting means the run is left for a person — the safe
    /// direction, because the other one restarts an agent behind somebody's back.
    recovery: Arc<Mutex<BTreeMap<RunId, Recovering>>>,
    /// When each of the account's rate limits lifts, as the agent last said (ADR-0029).
    ///
    /// Keyed by the agent's own `rateLimitType` — `five_hour`, `weekly`, whatever it invents
    /// next — because the limits are independent and the one that bites is the latest of them.
    /// In memory and never written down, for attendance's reason (ADR-0013): it is a moving
    /// value, and the only thing that makes a stale copy safe here is that every entry carries
    /// the instant it stops being true. A restart forgets, which is the safe direction — the next
    /// turn's event says so again within seconds, and the wrong direction would be a daemon
    /// holding work on a limit that lifted while it was down.
    ///
    /// One account per daemon (ADR-0028), so it is not keyed by account. The residual is stated
    /// there: re-authenticating as somebody else while the daemon runs leaves the old account's
    /// block applying to the new one until its reset, which holds work rather than spending it.
    limits: Arc<Mutex<BTreeMap<String, Millis>>>,
    /// Which peers have recently asked for a run's log, and when.
    ///
    /// A remote follower is a peer that keeps asking (`ClusterMessage::FetchEvents`), so being
    /// asked *is* the observation that somebody is watching — the same fact a local subscriber
    /// provides, arriving by a different route. In memory beside the local subscriber count for
    /// the same reason: attendance is knowable only where the stream is served, so there is
    /// nothing here to gossip and nobody to own it but this node.
    watchers: Arc<Mutex<BTreeMap<(RunId, NodeId), std::time::Instant>>>,
    /// Runs this node has already said would miss their deadline, and which deadline it was.
    ///
    /// The latch behind [`Supervisor::note_overdue`]. Keyed by the instant the run was due
    /// rather than by a bool, so **an operator who moves the deadline gets a fresh answer**:
    /// the alarm is about one stated time, and extending it is exactly the intervention that
    /// makes the old announcement stale. Nothing else clears it, because nothing else changes
    /// the fact.
    ///
    /// In memory, for the same reason attendance is: this is a note about having *told
    /// somebody*, which belongs to whoever did the telling. A restart says it again, which is
    /// at-least-once and the right way round — the other reading is a run that missed its
    /// deadline while a daemon was restarting and never mentioned it to anybody.
    overdue: Arc<Mutex<BTreeMap<RunId, Millis>>>,
    /// Where a fresh checkpoint gets copied so it survives this machine (ADR-0016).
    ///
    /// Set after the mesh starts, because the mesh needs the store the supervisor already
    /// holds, and `None` for as long as this node is alone — which is a state, not a fault:
    /// a fleet of one has nowhere to put a replica and says so rather than pretending.
    peers: Arc<std::sync::OnceLock<Arc<dyn Peers>>>,
    /// What this device is, as of the last probe — for the one thing about it that a *start*
    /// has to ask: how many sessions this agent install sustains.
    ///
    /// Handed in rather than probed here, because probing shells out and this is asked on the
    /// path that spawns an agent. `None` is tests only; see [`Supervisor::start_refusal`].
    capabilities: Arc<std::sync::OnceLock<Arc<crate::deliver::Current>>>,
    /// Questions this node's agents are blocked on (ADR-0017).
    ///
    /// In memory for a stronger reason than attendance's: a pending question exists exactly as
    /// long as the process blocked on it, and if this daemon dies the agent dies with it. A
    /// durable copy would be a list of questions nobody could still answer. The *record* is
    /// durable — `LogKind::Asked` and `LogKind::Answered` are in the run's log.
    asks: Arc<crate::asks::Asks>,
    /// The machine's reservation ledger, shared with every other `offloadd` on this device
    /// (ADR-0013's broker, [`crate::broker`]).
    ///
    /// `None` when it could not be opened, which is a degraded mode rather than a fault: the
    /// alternative is a daemon that will not start because another fleet's ledger is
    /// unreadable, and the failure it guards against — two fleets over-committing one laptop —
    /// is the situation this device was in before the ledger existed at all. It says so loudly
    /// once, at startup.
    broker: Option<crate::broker::Broker>,
}

/// The mesh, as the supervisor sees it: somewhere to put a copy of a checkpoint, and
/// somewhere to get one back.
///
/// What happened to a checkpoint that wanted a second home.
///
/// Three outcomes rather than two, because "no copy was made" is not one fact: a fleet of one
/// has nobody to ask and is fine, a peer refusing is worth a sentence, and a blob over the
/// protocol's cap will be refused by every node for ever and is the only one an operator has to
/// act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replicated {
    /// This peer holds a copy now.
    To(NodeId),
    /// Nobody eligible to ask — a fleet of one, or no available peer.
    NoPeer,
    /// The peer that was asked would not take it. `permanent` marks a refusal that no retry and
    /// no other peer can change.
    Refused {
        peer: NodeId,
        why: String,
        permanent: bool,
    },
}

impl Replicated {
    /// The peer that took it, for callers that only want that.
    #[must_use]
    pub fn peer(&self) -> Option<NodeId> {
        match self {
            Replicated::To(peer) => Some(*peer),
            _ => None,
        }
    }
}

/// A trait so the supervisor does not depend on the mesh — and so a test can answer both
/// questions without a network.
#[async_trait::async_trait]
pub trait Peers: Send + Sync + 'static {
    /// Copy every blob of a checkpoint to one peer, and say which one took them — or why none
    /// did.
    ///
    /// All or nothing per peer: a node holding two of a checkpoint's three blobs cannot
    /// materialise the run, so counting it as a replica would make `is_durable` a lie.
    ///
    /// Answers [`Replicated`] rather than `Option<NodeId>`. The `None` was every reason at once
    /// — a fleet of one, a peer that refused, a transport error, a blob the protocol will not
    /// move — and the operator-facing sentence it produced (*"checkpoint is on this node only;
    /// nobody would take a copy"*) reads as *nobody was available*, which is the one cause that
    /// gets better on its own. Measured in session seventy-eight against a 256 MiB bundle with
    /// the cap lowered under it: the peer was alive, meshed and willing, and the only record of
    /// the real reason was a `tracing::debug!` inside `offload-cluster`.
    async fn replicate(&self, run: RunId, blobs: Vec<offload_core::BlobHash>) -> Replicated;

    /// Get a blob from whoever has it. Verified by hash on arrival, so *whoever* is a
    /// performance question rather than a trust one (ADR-0016).
    async fn fetch(&self, blob: offload_core::BlobHash) -> Result<(), String>;

    /// Which services some *other* node offers a run the use of (ADR-0011).
    ///
    /// Read from the view and therefore a tick stale, which is adequate here in a way it is not
    /// for placement: being wrong costs one refused tool call rather than a misplaced run. A
    /// default of "nobody" so a fleet of one — and every test that only cares about blobs —
    /// answers correctly without saying anything.
    fn resources_elsewhere(&self) -> crate::resource::Reachable {
        crate::resource::Reachable::default()
    }
}

impl std::fmt::Debug for Supervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supervisor")
            .field("node_id", &self.node_id.short())
            .finish_non_exhaustive()
    }
}

/// Recover from a poisoned lock rather than panicking. A previous panic while holding
/// this lock should not take the whole daemon down with it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[must_use]
pub fn now() -> Millis {
    Millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
    )
}

/// What a pass over the failed runs did.
///
/// Two outcomes rather than one list, because they are two different things to say and the
/// caller says both. `resumed` is this node picking its own work back up (ADR-0013); its runs
/// are returned whole so whoever has the cluster can gossip them. `handed_back` is this node on
/// its way out giving work to somebody else (ADR-0043), which needs no run — the record is
/// already in the store and goes out with every other one — only a number to report.
#[derive(Debug, Default)]
pub struct Recovered {
    pub resumed: Vec<Run>,
    pub handed_back: Vec<Run>,
}

/// What a node remembers about retrying one failed run.
///
/// A snapshot for a *report*, which is why it is a separate type from `Recovering`: that one is
/// live state under a lock and this one crosses the control socket as sentences.
#[derive(Debug, Clone, Copy)]
pub struct RecoveryState {
    /// Who was watching **when it failed** — the observation the decision was taken on. `None`
    /// where this node cannot say, which is a real answer and not a missing one.
    pub observed: Option<offload_core::Attendance>,
    /// How many times this node has picked it up.
    pub resumes: u32,
    /// The decision, once it has been taken to leave the run for a person.
    pub decided: Option<offload_core::Escalation>,
}

/// What the worktree column says about a run that ended without ever starting.
///
/// Not blank, and not the refusal it was waiting under. A held run publishes *why* it has not
/// started into this column ([`summarise_refusal`]), and that reason is a sentence about a moment
/// — left in place once the run is over it becomes the fleet's standing answer about a run that
/// is not waiting for anything. What somebody wants there instead is the thing the column is
/// actually for: whether there is a checkout, and there is not.
const NEVER_STARTED: &str = "never started";

/// A refusal in the few words the gossiped worktree summary has room for.
///
/// Two things it must not be, and the long form is both. `offload ps`'s `WORKSPACE` column is
/// **18 characters** — the summary beside it is `clean`, `2 modified, 1 new`, `removed`, and the
/// longest of them, `waiting for a slot`, is exactly eighteen — and a sixty-character sentence
/// there pushes every following column off the end of the table, which is the misalignment
/// CLAUDE.md already has an entry about. Twenty overflows it too, which is what the first attempt
/// at this did; there is a test below that measures every string here against the column. And this string **gossips and is
/// stored**, so it must carry no duration: "for another 38.2s" is a sentence about a moment read
/// back later as a claim about now, which is `offload rules`' bug and `offload explain`'s.
///
/// The number belongs where it is computed fresh — `held back` in `offload explain`, and the
/// `rate-limited` line in `offload status`.
#[must_use]
pub fn summarise_refusal(refusal: &offload_core::Refusal) -> String {
    match refusal {
        offload_core::Refusal::AccountRateLimited { .. } => "rate-limited".to_string(),
        offload_core::Refusal::AccountAtCapacity { .. } => "account is full".to_string(),
        offload_core::Refusal::AgentAtCapacity(_) => "agent is full".to_string(),
        offload_core::Refusal::AtCapacity { .. } => "waiting for a slot".to_string(),
        offload_core::Refusal::BudgetFull { .. } => "waiting for budget".to_string(),
        // Everything else refuses a run rather than holding one, so it does not reach a summary —
        // and if a new variant ever does, "waiting for a slot" is what this column said before
        // any of them were named, which is the safe wrong answer rather than a misleading one.
        _ => "waiting for a slot".to_string(),
    }
}

/// What an edit did to a run that is **accepted here and not started** — the queue note.
///
/// Its own function so it can be tested, and because the sentence it replaced was a guess about
/// which state the run was in. `at` is the run's index after the edit in the order
/// [`Supervisor::held_but_not_started`] produces, and `of` is how many are waiting; both come
/// from that function rather than from a second reading of `urgency_order`.
///
/// The clause each edit needs is different, and neither is obvious:
///
/// * A **deadline** moves the run outright, because slack leads. The surprise worth printing is
///   the direction: an unstated deadline means *as soon as you can*, so it sorts ahead of every
///   run with a time on it — giving a queued run a deadline usually sends it **backwards**.
/// * A **priority** moves it only against runs with the same slack, which for runs submitted at
///   different moments with no deadline is none of them. An operator who is not told that has
///   typed a command that did nothing.
fn queue_note(edit: offload_core::SpecEdit, at: usize, of: usize) -> String {
    let place = if at == 0 {
        format!("first of the {of} waiting for a slot here, so it is the one this node starts next")
    } else {
        format!("{} of the {of} waiting for a slot here", ordinal(at + 1))
    };
    let why = match (edit, at) {
        (offload_core::SpecEdit::Deadline { .. }, 0) => String::new(),
        (offload_core::SpecEdit::Deadline { .. }, _) => {
            " — the ones ahead are due sooner, and a run with no stated deadline is due as soon \
             as it can be, which is sooner than any time you can name"
                .to_string()
        }
        (offload_core::SpecEdit::Priority { .. }, 0) => String::new(),
        (offload_core::SpecEdit::Priority { .. }, _) => {
            " — urgency leads and priority only breaks its ties, so this moves it past nothing \
             that is more overdue"
                .to_string()
        }
    };
    format!("it has not started yet: it is now {place}{why}")
}

/// `1st`, `2nd`, `3rd`, `4th` — for a position in a queue somebody is reading.
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A refusal as an operator should read it, with a clock applied where one is needed.
///
/// Only one variant needs it, and it needs it badly: `Refusal::AccountRateLimited` carries the
/// instant the limit lifts, `offload-core` has no clock (ADR-0001), and `Millis`'s `Display`
/// renders a duration — so the instant printed through it came out as `496117104h5m`. Everything
/// else defers to the type's own sentence, which is where those belong.
#[must_use]
pub fn describe_refusal(refusal: &offload_core::Refusal) -> String {
    match refusal {
        offload_core::Refusal::AccountRateLimited { until } => format!(
            "the account's rate limit is holding new runs for another {}",
            until.saturating_sub(now())
        ),
        other => other.to_string(),
    }
}

/// How long a finished run may wait to be told to the fleet (see `Supervisor::runs_to_publish`).
/// A day: long enough for a laptop on a flight, short enough that a restart's one republishing
/// exchange stays small.
pub const UNTOLD_HORIZON: offload_core::Millis = offload_core::Millis(24 * 60 * 60 * 1_000);

/// The most run records one node publishes in a tick, live ones first.
const PUBLISH_CAP: usize = 64;

/// Which finished runs have been out in a gossip exchange yet.
#[derive(Debug, Default)]
struct Untold {
    /// First seen finished, with the exchange count at that moment.
    waiting: std::collections::HashMap<RunId, u64>,
    /// Out in at least one exchange since.
    told: std::collections::HashSet<RunId>,
}

/// The agent this node's runs are spawned with: the owner's binary, under the owner's account.
///
/// One function because two things start it, a run and the model list's read (ADR-0080), and
/// the model list is per account: a read that found the agent by another route would advertise
/// a different login's models.
#[must_use]
pub fn agent_for(config: &Config) -> ClaudeCode {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config.state_dir.clone());
    // The owner's nominated state directory wins over the environment (ADR-0028): it is what
    // selects the account, and the probe was handed the same path, so the two cannot report
    // different logins for one node.
    match &config.agent.config_dir {
        Some(dir) => ClaudeCode::with_config_dir(dir.clone()),
        None => ClaudeCode::new(home),
    }
    .with_binary(config.agent.binary.clone())
}

impl Supervisor {
    pub fn new(config: Arc<Config>, node_id: NodeId, store: Store) -> Self {
        let workspaces =
            WorkspaceManager::with_checkouts(config.state_dir.clone(), config.checkouts_dir());
        let agent = agent_for(&config);
        // Asked of the adapter rather than computed again: where an agent keeps its state is the
        // adapter's business (ADR-0004), and it has already resolved it — including the case
        // where the owner moved it with `$CLAUDE_CONFIG_DIR`, which is the difference between
        // checkpoints working on this machine and failing every single time.
        let home = agent.config_dir().to_path_buf();
        Supervisor {
            config,
            node_id,
            workspaces,
            agent,
            home,
            store,
            live: Arc::new(Mutex::new(BTreeMap::new())),
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            revoked: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            untold: Arc::new(Mutex::new(Untold::default())),
            recovery: Arc::new(Mutex::new(BTreeMap::new())),
            limits: Arc::new(Mutex::new(BTreeMap::new())),
            watchers: Arc::new(Mutex::new(BTreeMap::new())),
            overdue: Arc::new(Mutex::new(BTreeMap::new())),
            peers: Arc::new(std::sync::OnceLock::new()),
            capabilities: Arc::new(std::sync::OnceLock::new()),
            asks: Arc::new(crate::asks::Asks::default()),
            broker: match crate::broker::Broker::open() {
                Ok(broker) => {
                    tracing::debug!(path = %broker.path().display(), "device reservation ledger");
                    Some(broker)
                }
                Err(e) => {
                    // Loud, because the consequence is silent: this device goes back to
                    // over-committing itself across fleets, and nothing else would say so.
                    tracing::warn!(
                        error = %e,
                        "no device capacity ledger — a second fleet on this machine cannot see \
                         what this one has promised"
                    );
                    None
                }
            },
        }
    }

    /// Where the agent this node spawns keeps its state — the directory that selects the account.
    ///
    /// Asked of the adapter, which is the one that resolved it, for the reason `Supervisor::new`
    /// gives about not computing the same path twice: a report that derived it again could name a
    /// directory the agent is not using.
    #[must_use]
    pub fn agent_state_dir(&self) -> &std::path::Path {
        self.agent.config_dir()
    }

    /// Hand the supervisor the mesh. Called once, after it is up.
    pub fn peers_via(&self, peers: Arc<dyn Peers>) {
        let _ = self.peers.set(peers);
    }

    /// Hand the supervisor this device's live capabilities. Called once, at startup.
    pub fn capabilities_via(&self, capabilities: Arc<crate::deliver::Current>) {
        let _ = self.capabilities.set(capabilities);
    }

    /// How many runs this device's agent install sustains at once, if anything has said.
    ///
    /// The probe's number, which `deliver::set_agent_concurrency` replaces with the owner's
    /// `agent.max_concurrent` where they gave one — so this is the same value the bid round reads
    /// out of `Capabilities` and the same one `offload status` prints as *"claude-code sustains
    /// N, which is what binds"*.
    /// Is this run's agent already at the ceiling its install sustains?
    ///
    /// **The clause every door that starts an agent was missing.** `WorkPolicy::admits` has it
    /// and `Room::for_one_more` does not, and `admits` is called by exactly one thing — the bid
    /// round. So the number `offload status` prints as *"claude-code sustains 2, which is what
    /// binds"* shaped what a node **bid** and stopped nothing. Measured on a fleet of one with no
    /// cluster at all, where `admits` is never reached: three submissions accepted without a word,
    /// `runs 3/3 · claude-code sustains 2, which is what binds`, and three agent processes.
    /// `Refusal::AgentAtCapacity` could be computed and could not stop anything.
    ///
    /// Three doors ask it now, and the **count** each passes is its own because they mean
    /// different things: a submission asks what this node has *committed* (`held_for_agent`), a
    /// start asks what is *going* (`started_excluding_on_agent`), and `resume` is a submission's
    /// question about a run that already exists. That is the same split `Room::for_one_more`'s
    /// callers already draw for the machine's own capacity, and passing the count in is what keeps
    /// this from becoming a third opinion about which runs to count. `restart_task` is the fourth
    /// caller of that shape and needs nothing: a task names no agent, so this answers `None`.
    ///
    /// `None` from [`Self::agent_ceiling`] is "nothing has told this supervisor what the device
    /// is", which is tests and nothing else — the daemon wires it before it serves. It must not
    /// become a silent pass in production: if another construction path appears, this is the line
    /// that goes quiet.
    fn agent_full(
        &self,
        run: &Run,
        count: impl Fn(&offload_core::AgentKind) -> u32,
    ) -> Option<offload_core::Refusal> {
        let kind = run.spec.agent_kind()?;
        let ceiling = self.agent_ceiling(kind)?;
        (count(kind) >= ceiling).then(|| offload_core::Refusal::AgentAtCapacity(kind.clone()))
    }

    fn agent_ceiling(&self, agent: &offload_core::AgentKind) -> Option<u32> {
        self.capabilities
            .get()?
            .now()
            .agent_details(agent)
            .map(|details| details.max_concurrent)
    }

    /// Point the supervisor at a different agent config directory.
    ///
    /// It decides where transcripts are read from and written to, so anything that wants to
    /// exercise checkpoint or resume without touching the real one sets it here.
    #[must_use]
    pub fn with_home(mut self, config: PathBuf) -> Self {
        // Explicit rather than derived: a test's temporary directory must not be quietly
        // replaced by whatever `$CLAUDE_CONFIG_DIR` says on the machine running the suite.
        self.agent = ClaudeCode::with_config_dir(config.clone())
            .with_binary(self.config.agent.binary.clone());
        self.home = config;
        self
    }

    /// Use a private device ledger instead of this machine's.
    ///
    /// [`with_home`](Self::with_home)'s sibling, and for the same reason one line further out:
    /// the device reservation ledger is machine-wide *by design* — ADR-0013's one laptop, two
    /// fleets, one set of slots — so [`Supervisor::new`] finds the real one. Which means every
    /// capacity test read whatever `offloadd` happened to be running on the machine executing
    /// the suite. Four of them failed whenever the demo was up, with
    /// `two_commitments_on_a_one_slot_node_do_not_block_each_other` reporting `left: 0, right:
    /// 1` about a slot an unrelated agent was holding — and the obvious suspect is whatever you
    /// had just changed. It runs the other way too: a test that drives a run *reserves* against
    /// that ledger, so `cargo test` could tell a live daemon its machine was full.
    ///
    /// In memory, which is the isolation these tests already have from `Store::open_memory`.
    #[must_use]
    pub fn with_private_ledger(mut self) -> Self {
        self.broker = crate::broker::Broker::open_memory().ok();
        self
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Reconcile the store with reality at startup.
    ///
    /// Any run the store thinks is *held* was being hosted by a daemon that is no longer
    /// running — its agent process died with it. Leaving those rows as `running` would
    /// make `ps` claim work is happening that is not, which is worse than an honest
    /// failure. Phase 4 replaces this with `Orphaned` plus the hold-down policy, once
    /// another node exists that could actually pick the run up.
    ///
    /// A `Pending` run is deliberately left alone: it has no holder and no agent process,
    /// so a restart tells us nothing new about it. That is the state `offload checkpoint`
    /// produces, and failing it here would destroy the runs most worth keeping.
    pub fn recover(&self) -> Result<usize, SubmitError> {
        let mut count = 0;
        for run in self.store.list_runs(false)? {
            if matches!(run.state, RunState::Pending { .. }) {
                continue;
            }
            // Only ours to give up on. With a mesh this store also holds runs placed on other
            // nodes, and failing one of those because *we* restarted would tell the fleet a
            // machine that is happily working has lost the run.
            if run.holder() != Some(self.node_id) {
                continue;
            }
            // Point at the way back before the user has to go looking for one: a run with
            // a checkpoint has its conversation and its uncommitted edits intact, and the
            // difference between "failed" and "failed but resumable" is the whole reason
            // for taking checkpoints at all.
            let reason = match &run.checkpoint {
                Some(cp) => format!(
                    "daemon restarted while this run was active — resumable from turn {} \
                     with `offload resume {}`",
                    cp.turns,
                    run.id.short()
                ),
                None => "daemon restarted while this run was active".to_string(),
            };
            // Through the row, like every other writer here — not because anything else is
            // running yet (this is startup, before a single task is spawned) but because a
            // listing followed by a write is the shape that was wrong everywhere else, and one
            // exception is how it comes back.
            self.store
                .update_run(run.id, |run| run.abandon(reason, now()))?;
            self.note_failure_seen_as(run.id, None);
            count += 1;
            tracing::warn!(run_id = %run.id, "marked interrupted run as failed on startup");
        }
        if count > 0 {
            tracing::warn!(count, "recovered interrupted runs from a previous daemon");
        }
        Ok(count)
    }

    /// Copy the run's newest checkpoint somewhere else, in the background.
    ///
    /// Spawned rather than awaited: the agent's next turn does not wait for the network
    /// (ADR-0016), and a slow peer must not be able to slow a run down. The run record is
    /// updated when the copy is confirmed, so `offload ps` can say whether the work would
    /// survive this machine going away.
    ///
    /// [`Self::replicate_now`] is the same work with the result waited for. Exactly one caller
    /// wants that and it is the one place where the trade-off inverts: a node on its way out is
    /// not protecting a turn that is about to happen, it is spending its last seconds
    /// (ADR-0054).
    fn replicate(&self, run_id: RunId, checkpoint: Checkpoint) {
        let this = self.clone();
        tokio::spawn(async move {
            this.replicate_now(run_id, checkpoint).await;
        });
    }

    /// [`Self::replicate`], awaited — and answering which peer took it.
    ///
    /// Both early returns belong here rather than at the call sites: a node with no fleet has
    /// nobody to ask, and a checkpoint somebody else already holds does not need copying. The
    /// second is why this is cheap to call over every run on the way out.
    async fn replicate_now(&self, run_id: RunId, checkpoint: Checkpoint) -> Option<NodeId> {
        let peers = self.peers.get().cloned()?;
        if checkpoint.is_durable(Some(self.node_id)) {
            return None;
        }
        let store = self.store.clone();

        // The checkpoint's own blobs, **and the workspace archive if there is one**.
        //
        // Not part of the checkpoint — `Checkpoint::blobs` is what the checkpoint is made *of*,
        // and putting the archive in there would make it lie — but it is part of what a peer
        // needs to **materialise** this run, which is the question replication exists to answer.
        // Without it a run reported `replicated` was not: measured in session seventy-eight on
        // two daemons, where bravo held every checkpoint blob, alpha was killed, bravo took the
        // run over exactly as designed, and it failed at turn 12 with *"no peer could supply the
        // blob"*. Twelve turns lost on a run whose SAFE column had promised it would survive.
        //
        // Here rather than beside `is_durable`, because this makes that honest for free:
        // `Mesh::replicate` is all-or-nothing per peer, so a peer that could not take the
        // archive is never recorded as a replica and the column stops promising what the fleet
        // cannot do.
        //
        // Offered every turn and moved once — ADR-0061 §3's *once per node per run* — because
        // the second offer of an archive a peer already holds is declined as `already held`
        // before any bytes go anywhere.
        let mut blobs = checkpoint.blobs();
        if let Some(archive) = store
            .load_run(run_id)
            .ok()
            .flatten()
            .and_then(|run| run.spec.workspace().map(|w| RepoSource::parse(&w.repo)))
            .and_then(|source| source.archive())
        {
            blobs.push(archive);
        }

        {
            match peers.replicate(run_id, blobs).await {
                Replicated::To(peer) => {
                    // Applied to the row rather than to the copy above: turns keep happening
                    // while bytes are in flight, and writing back a stale checkpoint would undo
                    // one. This is where that was written down first, and it read the row again
                    // to find out — which is the same thing one lock later, except that the
                    // window between the read and the write has stopped existing.
                    let noted = store.update_run(run_id, |current| {
                        let still_current = current
                            .checkpoint
                            .as_ref()
                            .is_some_and(|c| c.taken_at == checkpoint.taken_at);
                        if !still_current {
                            return false;
                        }
                        if let Some(c) = current.checkpoint.as_mut() {
                            c.replicas.insert(peer);
                        }
                        true
                    });
                    match noted {
                        Ok(Some(true)) => {
                            tracing::info!(run_id = %run_id, node = %peer.short(), "checkpoint replicated");
                        }
                        // The run moved on while the bytes were in flight, or went away: the
                        // copy is still there and still useful, it is simply not this run's
                        // newest one any more.
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(run_id = %run_id, error = %e, "could not record the replica");
                        }
                    }
                    Some(peer)
                }
                // A fleet of one has nobody to ask, and that is not a problem to report at
                // `warn` every turn — it is the ordinary state of a single-node fleet.
                Replicated::NoPeer => {
                    tracing::debug!(
                        run_id = %run_id,
                        "checkpoint is on this node only; no peer to offer it to"
                    );
                    None
                }
                // A peer that refused is worth the sentence it gave, and a *permanent* refusal
                // is worth saying so: nothing about waiting, retrying or a different peer will
                // change it, so the old line's implicit "we will try again" was a promise the
                // node could not keep.
                Replicated::Refused {
                    peer,
                    why,
                    permanent,
                } => {
                    if permanent {
                        tracing::warn!(
                            run_id = %run_id,
                            node = %peer.short(),
                            reason = %why,
                            "checkpoint is on this node only and no retry will change that"
                        );
                    } else {
                        tracing::warn!(
                            run_id = %run_id,
                            node = %peer.short(),
                            reason = %why,
                            "checkpoint is on this node only; the peer would not take a copy"
                        );
                    }
                    None
                }
            }
        }
    }

    /// Push the last copy of every checkpoint that has only one, on the way out (ADR-0054).
    ///
    /// **The one moment the fleet is allowed to wait for the network.** Everywhere else
    /// replication is spawned and unwaited on purpose (ADR-0016): the agent's next turn must not
    /// wait on a slow peer. Here there is no next turn — this node is leaving — and the thing
    /// being protected is the only copy of a conversation.
    ///
    /// ADR-0043 left this deliberately, and two measurements since then moved the question.
    /// `Supervisor::replicate` has one caller, `checkpoint`, with no retry and no background
    /// pass, so a capture taken while nobody was reachable stays `here only` **for ever** — a
    /// peer arriving later fixes nothing. A *failed* run has no next checkpoint to fix it, and a
    /// `Pending` one released by `offload checkpoint` has no next turn. So "the copy was never
    /// made because there was no peer" and "there is a peer now" can both be true at the moment
    /// this node departs, and until this pass existed nobody joined them up.
    ///
    /// Bounded as a whole rather than per run: a drain has a deadline and this runs before the
    /// handover pass, so the honest failure is *some* runs unattempted with a line saying so,
    /// never a departure that hangs. Every failure is soft — a run whose copy could not be
    /// pushed is exactly as stranded as it was before this function existed, and says so through
    /// the same `NoCopyElsewhere` sentence.
    pub async fn push_last_copies(&self, budget: Duration) -> LastCopies {
        let mut out = LastCopies::default();
        if self.peers.get().is_none() {
            return out;
        }
        // **`true`, and the flag is the whole point of this pass.** A `Failed` run is *terminal*,
        // so `list_runs(false)` omits exactly the rows this exists for — the same trap
        // `Checkpoint::is_durable`'s caller fell into, written down in
        // `docs/pitfalls/checkpoints-blobs-and-workspaces.md` as "`is_terminal` is the wrong
        // question in a report for the same reason": a failed run is terminal *and* reopens.
        // `resumable_checkpoint` below is the filter that is right, because its arms are written
        // out and a new terminal state has to decide.
        let Ok(runs) = self.store.list_runs(true) else {
            // Unknown is not none: a store that will not answer is not a store with nothing to
            // copy, and the drain must not read a failed query as "everything is safe".
            tracing::warn!("could not read the run list; last copies were not attempted");
            return out;
        };

        let deadline = Instant::now() + budget;
        for run in runs {
            // `resumable_checkpoint`, not the field, and the same reason the collector uses it:
            // a `Completed` or `Cancelled` run's capture is a note about the past that nothing
            // can start from, so copying it is bytes spent on work nobody will do.
            let Some(checkpoint) = run.resumable_checkpoint() else {
                continue;
            };
            // The holder is this node for a run still assigned here and `None` for one that
            // failed or was released, so this is the plain question in both cases: does anybody
            // *else* have it.
            if checkpoint.is_durable(run.holder()) {
                out.already += 1;
                continue;
            }
            let Some(left) = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
            else {
                out.unattempted += 1;
                continue;
            };
            match tokio::time::timeout(left, self.replicate_now(run.id, checkpoint.clone())).await {
                Ok(Some(_)) => out.pushed += 1,
                Ok(None) => out.refused += 1,
                Err(_) => {
                    tracing::warn!(run_id = %run.id, "ran out of time pushing the last copy");
                    out.unattempted += 1;
                }
            }
        }
        if out.pushed > 0 || out.refused > 0 || out.unattempted > 0 {
            tracing::info!(
                pushed = out.pushed,
                refused = out.refused,
                unattempted = out.unattempted,
                already = out.already,
                "pushed the last copy of what only this node had"
            );
        }
        out
    }

    /// What is at `path` in a run's workspace on this node, read-only (ADR-0075): the checkout if
    /// it is here, else the run's branch in the mirror. In words when neither is.
    pub async fn peek(
        &self,
        run_id: RunId,
        path: &str,
    ) -> Result<offload_proto::cluster::FilesView, String> {
        use offload_proto::cluster::{FileEntry, FilesContent, FilesSource, FilesView};
        let run = self
            .run(run_id)
            .ok_or_else(|| format!("this node has no record of run {}", run_id.short()))?;
        let work = run
            .spec
            .agent()
            .ok_or_else(|| "a program run has no workspace to show".to_string())?;
        let source = RepoSource::parse(&work.workspace.repo);
        let branch = work
            .workspace
            .branch
            .clone()
            .unwrap_or_else(|| offload_workspace::run_branch(run_id));
        let peek = self.workspaces.peek(run_id, &source, &branch, path).await?;
        let content = match peek.shown {
            offload_workspace::peek::Shown::Listing(entries, truncated) => FilesContent::Listing {
                entries: entries
                    .into_iter()
                    .map(|(name, dir, bytes)| FileEntry { name, dir, bytes })
                    .collect(),
                truncated,
            },
            offload_workspace::peek::Shown::Text(text, bytes, truncated) => FilesContent::Text {
                text,
                bytes,
                truncated,
            },
            offload_workspace::peek::Shown::Binary(bytes) => FilesContent::Binary { bytes },
        };
        Ok(FilesView {
            path: path.trim_matches('/').to_string(),
            source: if peek.from_checkout {
                FilesSource::Checkout
            } else {
                FilesSource::Branch
            },
            node: self.fleet_name(),
            content,
        })
    }

    /// One run, as the store has it.
    #[must_use]
    pub fn run(&self, run_id: RunId) -> Option<Run> {
        self.store.load_run(run_id).ok().flatten()
    }

    /// The leg that produced this run's numbers: the node, and the epoch it held it under.
    ///
    /// Written for `RunProgress::absorb`'s ranking, and it answers a second question the record
    /// cannot: **which machine has this run's log**. A finished run names no node — the lease goes
    /// with the terminal transition — while its position was last written by the leg that ran it,
    /// which is by construction the leg that wrote the log.
    #[must_use]
    pub fn progress_leg(&self, run_id: RunId) -> Option<NodeId> {
        self.store
            .load_stats(run_id)
            .ok()
            .and_then(|stats| stats.by)
    }

    /// The fleet's one agreed sentence about this run's worktree.
    ///
    /// [`Self::progress_leg`]'s sibling off the same row, and the reason it is here rather than
    /// read at each call site: the note and the leg are two halves of one answer — *whose disk*
    /// and *what is on it* — and a caller that had the second without the first would be reading
    /// a claim without knowing whose. It is written only by the leg the checkout is on
    /// ([`Self::note_workspace`]), so it says `removed` on every node in the fleet once that leg
    /// has torn one down.
    #[must_use]
    pub fn workspace_note(&self, run_id: RunId) -> Option<String> {
        self.store
            .load_stats(run_id)
            .ok()
            .map(|stats| stats.workspace)
            .filter(|note| !note.is_empty())
    }

    /// What this node recorded becoming of a run's checkout, if it recorded anything.
    ///
    /// [`Self::workspace_note`]'s counterpart for the fact the *record* cannot carry. A worktree
    /// that is gone with no `removed` note went either through the sweep — which writes
    /// [`Self::note_reclaimed`] — or through nothing at all, and the two need opposite sentences:
    /// one says what became of it, the other says that nobody here knows. Read here rather than
    /// pointed at, because a report that names a command is a promise that command has an answer.
    #[must_use]
    pub fn reclaimed_here(&self, run_id: RunId) -> Option<offload_core::Reclamation> {
        self.store
            .audit(Some(run_id), 100)
            .ok()?
            .into_iter()
            .find_map(|entry| match entry.kind {
                offload_core::AuditEvent::Reclaimed { why, .. } => Some(why),
                _ => None,
            })
    }

    /// Does this node already hold a blob? Feeds `LocalFacts::checkpoint_local` at bid time.
    #[must_use]
    pub fn holds_blob(&self, hash: offload_core::BlobHash) -> bool {
        self.store.has_blob(hash)
    }

    #[must_use]
    pub fn workspaces(&self) -> &WorkspaceManager {
        &self.workspaces
    }

    /// How long a finished run keeps being gossiped after it ends.
    ///
    /// Two readers, and the second is the reason it has a name. [`Self::gossipable_runs`] uses it
    /// to stop publishing a run the fleet has heard about; ADR-0021's prune uses it as the quiet
    /// period a record has to sit through before it may be deleted, because a deletion the fleet
    /// undoes is not a deletion — and the copy a peer teaches back arrives through the merge
    /// path, which knows nothing about rules and would therefore hand back an *untagged* record
    /// that nothing can ever prune again. Two numbers that agree only because somebody remembered
    /// are two things to remember.
    pub const GOSSIP_TAIL: Millis = Millis(5 * 60 * 1_000);

    /// Runs worth telling the fleet about.
    ///
    /// Everything this node knows that is still live, plus what finished recently. *Not*
    /// filtered to runs this node holds: a terminal state has no holder, so filtering that
    /// way would silence the node exactly as it learned the news worth spreading — which is
    /// how a completed run sat at `running` on the machine that submitted it.
    ///
    /// Live runs are bounded by the fleet's concurrency caps, so the ordinary payload is a
    /// handful of records. Finished ones ride along for a few minutes; beyond that a node
    /// which was away shows a stale state until it asks, which is a real limit and a cheap
    /// one compared with gossiping every run for ever.
    #[must_use]
    pub fn gossipable_runs(&self) -> Vec<Run> {
        let now = now();
        let recently = Self::GOSSIP_TAIL;

        self.store
            .list_runs(true)
            .unwrap_or_default()
            .into_iter()
            .filter(|run| match run.state.finished_at() {
                None => true,
                Some(at) => now.saturating_sub(at) < recently,
            })
            .take(64)
            .collect()
    }

    /// Runs to publish this tick: what [`Self::gossipable_runs`] gives, plus every run that
    /// finished within [`UNTOLD_HORIZON`] and has not yet been out in a gossip exchange.
    ///
    /// `exchanges` is [`offload_cluster::Cluster::exchanges`], the count of exchanges in which
    /// this node's gossip reached a peer. A finished run is kept while that count has not moved
    /// since the run was first seen finished. The five-minute tail alone assumed somebody was
    /// listening when a run ended: the emulator cancelled a run before it had met anyone, met
    /// the fleet after the tail, and the laptop, the Mac and the tablet showed it `pending` for
    /// ever (session ninety-four). A run finished on a laptop that is offline for longer than
    /// the tail is the same hole. Live runs come first under the cap, so history can never
    /// crowd one out.
    #[must_use]
    pub fn runs_to_publish(&self, exchanges: u64) -> Vec<Run> {
        let now = now();
        let mut untold = lock(&self.untold);
        let mut live = Vec::new();
        let mut finished = Vec::new();
        let mut still = std::collections::HashSet::new();
        for run in self.store.list_runs(true).unwrap_or_default() {
            let Some(at) = run.state.finished_at() else {
                live.push(run);
                continue;
            };
            let age = now.saturating_sub(at);
            let waiting = if age >= UNTOLD_HORIZON {
                false
            } else {
                still.insert(run.id);
                if untold.told.contains(&run.id) {
                    false
                } else {
                    match untold.waiting.get(&run.id).copied() {
                        None => {
                            untold.waiting.insert(run.id, exchanges);
                            true
                        }
                        Some(since) if exchanges > since => {
                            untold.waiting.remove(&run.id);
                            untold.told.insert(run.id);
                            false
                        }
                        Some(_) => true,
                    }
                }
            };
            if waiting || age < Self::GOSSIP_TAIL {
                finished.push(run);
            }
        }
        untold.told.retain(|id| still.contains(id));
        untold.waiting.retain(|id, _| still.contains(id));
        let room = PUBLISH_CAP.saturating_sub(live.len());
        live.truncate(PUBLISH_CAP);
        live.extend(finished.into_iter().take(room));
        live
    }

    /// Publish a finished run again until the next exchange: a peer still has it as live.
    ///
    /// Forgetting that it was told makes the next [`Self::runs_to_publish`] take it up again,
    /// within the horizon, with the exchange count of that moment. A peer that keeps sending its
    /// stale copy keeps it published, which is the point: it stops once the peer has the news.
    pub fn retell(&self, run: RunId) {
        let mut untold = lock(&self.untold);
        untold.told.remove(&run);
        untold.waiting.remove(&run);
    }

    /// What the runs worth gossiping have done and cost.
    ///
    /// Only the ones this node *ran*: a record with all-zero numbers is the submitting node's
    /// copy of somebody else's run, and gossiping it would be this node asserting a fact it
    /// has no standing to state. The forward-only merge rule would refuse it at the far end,
    /// but a claim that has to be refused is one that should not have been made.
    #[must_use]
    pub fn gossipable_progress(&self) -> Vec<(RunId, offload_core::RunProgress)> {
        self.store
            .list_with_stats(true)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, stats)| *stats != offload_core::RunProgress::default())
            .map(|(run, stats)| (run.id, stats))
            .take(64)
            .collect()
    }

    /// Write down what a run elsewhere has done, if it is news.
    ///
    /// The same merge as the view's and not because the view already did it: the store outlives
    /// the view, and a restart that replayed an old peer's gossip into it would otherwise walk a
    /// finished run's numbers backwards on disk, where nothing later can correct them.
    ///
    /// Read-modify-write rather than an overwrite, because the rule combines: what survives is
    /// this row's spend folded with the winning leg's position, and neither record on its own
    /// is that answer.
    pub fn record_progress(&self, run: RunId, progress: &offload_core::RunProgress) {
        let _ = self.store.update_stats(run, |known| known.absorb(progress));
    }

    /// The runs this node is holding right now, for gossip.
    ///
    /// Read from the store rather than from memory, so it is the same answer `offload ps`
    /// gives — a node whose gossip disagrees with its own registry is the hardest kind of
    /// inconsistency to chase.
    ///
    /// **Held, not merely known**, which is the same sentence [`Self::held`] carries and for the
    /// same reason — and it was missing here, on the *gossiped* copy of the number. The store
    /// holds records for runs this node submitted elsewhere and, since ADR-0025, for work it
    /// neither ran nor submitted at all: a bystander keeps the record because it is the index
    /// `offload logs` resolves an id in. So every node that had merely *heard* of a run claimed
    /// to be running it, and `NodeView::running` — documented as "what this node says it is
    /// running", the ground truth a registry could be rebuilt from — said so about somebody
    /// else's machine. Measured on two daemons: one run, held by alpha, and `offload nodes`
    /// reported `RUNS 1` for both rows on both machines, where beta could not host a run at all
    /// (`accepting no — this node has not been granted host-runs`) and its own `offload status`
    /// said `runs 0/2` one command away. On a fleet of five, one run reads as five.
    #[must_use]
    pub fn active_runs(&self) -> std::collections::BTreeSet<RunId> {
        let me = self.node_id;
        self.store
            .list_runs(false)
            .map(|runs| {
                runs.into_iter()
                    .filter(|run| run.holder() == Some(me))
                    .map(|run| run.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Runs this node is holding: its load, and what a drain could not hand over.
    ///
    /// **Held, not merely known.** The store contains runs this node submitted and placed
    /// somewhere else — that is the point of recording them — and counting those as load is
    /// how a node that had submitted six runs to a peer reported `runs 6/2, at capacity` while
    /// sitting completely idle, and then declined to bid for anything. `held_count` was
    /// already right and `running_count` beside it was not; there is one of them now.
    ///
    /// Held includes `Assigned`: a run this node committed to and has not started yet occupies
    /// a slot, because that is what the commitment means (ADR-0006).
    ///
    /// Counted *and* weighed in one pass, so the number of runs and the shares they spend can
    /// never describe different sets (ADR-0013).
    #[must_use]
    pub fn held(&self) -> Occupancy {
        let me = self.node_id;
        let mut held = Occupancy::default();
        for run in self.store.list_runs(false).unwrap_or_default() {
            if run.holder() == Some(me) {
                held.runs = held.runs.saturating_add(1);
                held.shares = held.shares.saturating_add(run.spec.demand.shares());
            }
        }
        held
    }

    /// What *other* `offloadd` instances on this machine have promised (ADR-0013's broker).
    ///
    /// Added to this node's own occupancy at every capacity question, and deliberately not
    /// folded into [`Self::held`]: `offload ps` saying "runs 2/4" must keep meaning this node's
    /// runs, or a number an operator reads as theirs is partly somebody else's.
    ///
    /// Zero when there is no ledger, which is the old behaviour exactly — an unreadable ledger
    /// makes this device over-commit as it always did, rather than refuse work.
    pub fn device_elsewhere(&self) -> Occupancy {
        let Some(broker) = &self.broker else {
            return Occupancy::default();
        };
        match broker.committed_elsewhere(&self.instance(), now()) {
            Ok(occupancy) => occupancy,
            Err(e) => {
                tracing::warn!(error = %e, "could not read the device capacity ledger");
                Occupancy::default()
            }
        }
    }

    /// How this node names itself in the shared ledger.
    ///
    /// Its node id, which is opaque and stable. Deliberately not the configured name or anything
    /// naming a *fleet*: the file is readable by the machine's other instances, and ADR-0013
    /// permits them the number and nothing else.
    fn instance(&self) -> String {
        self.node_id.to_string()
    }

    /// Add what the rest of the device has promised to what this node holds.
    fn with_device(&self, mine: Occupancy) -> Occupancy {
        let theirs = self.device_elsewhere();
        Occupancy {
            runs: mine.runs.saturating_add(theirs.runs),
            shares: mine.shares.saturating_add(theirs.shares),
        }
    }

    /// Make the ledger say what this node is actually holding.
    ///
    /// A reconcile rather than a pair of reserve/release calls on the lifecycle paths, because
    /// the ledger and the store drift for ordinary reasons — a run finishes, migrates away, or
    /// this daemon is killed and restarts to find its own rows from a previous life still there
    /// — and a reconcile handles all of them, including the ones nobody thought of. It runs from
    /// the heartbeat because a reservation *is* a lease: whatever hands one out has to arrange
    /// for its renewal in the same breath, which is the rule a missing heartbeat already cost
    /// this project once.
    fn reconcile_reservations(&self, held: &[Run]) {
        let Some(broker) = &self.broker else {
            return;
        };
        let instance = self.instance();
        let now = now();
        let ttl = crate::broker::RESERVATION_TTL;

        for run in held {
            if let Err(e) = broker.reserve(&instance, run.id, run.spec.demand, now, ttl) {
                tracing::warn!(run_id = %run.id, error = %e, "could not reserve device capacity");
            }
        }

        // Anything the ledger still has for us that we are no longer holding. Expiry would get
        // there eventually; releasing now means the machine's other fleet stops being told this
        // device is busy a minute after it stopped being busy.
        match broker.mine(&instance, now) {
            Ok(promised) => {
                for id in promised {
                    if !held.iter().any(|run| run.id == id) {
                        if let Err(e) = broker.release(&instance, id) {
                            tracing::warn!(run_id = %id, error = %e, "could not release device capacity");
                        }
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not read this node's reservations"),
        }

        if let Err(e) = broker.sweep(now) {
            tracing::warn!(error = %e, "could not sweep expired reservations");
        }
    }

    /// What the runs held here are doing, one line each, by [`Self::held`]'s rule: for a host that
    /// says what it is busy with (ADR-0079), rather than only that it is busy.
    #[must_use]
    pub fn held_work(&self) -> Vec<String> {
        let me = self.node_id;
        self.store
            .list_runs(false)
            .unwrap_or_default()
            .iter()
            .filter(|run| run.holder() == Some(me))
            .map(|run| run.spec.work.summary())
            .collect()
    }

    /// Just the count, for the places that display it.
    #[must_use]
    pub fn held_count(&self) -> u32 {
        self.held().runs
    }

    /// Of the runs held here, how many have **no turn boundary** — tasks (ADR-0019 §2).
    ///
    /// The no-fleet drain arm counts from [`Self::held_count`] rather than from the pass that
    /// splits them, so without this a running task is `no_boundary` on one path and `left` on the
    /// other — one fact with two spellings, which is the trap `finished`, `later` and `pooled`
    /// were each split out of `left` to avoid.
    #[must_use]
    pub fn held_no_boundary(&self) -> u32 {
        let me = self.node_id;
        self.store
            .list_runs(false)
            .unwrap_or_default()
            .iter()
            .filter(|run| {
                run.holder() == Some(me) && run.spec.work.kind() != offload_core::WorkKind::Agent
            })
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    /// Held runs on one agent, for that agent's own per-node ceiling.
    ///
    /// Its own pass rather than a field on [`Self::held`], because the two answer different
    /// questions and the place that used to need this passed `held().runs` instead — every
    /// agent conflated, so `offload status` and `bid::evaluate` computed different numbers for
    /// the same rule.
    #[must_use]
    pub fn held_for_agent(&self, agent: &offload_core::AgentKind) -> u32 {
        let me = self.node_id;
        let mut count = 0u32;
        for run in self.store.list_runs(false).unwrap_or_default() {
            if run.holder() == Some(me) && run.spec.agent().is_some_and(|work| &work.agent == agent)
            {
                count = count.saturating_add(1);
            }
        }
        count
    }

    /// The caller's `Room`, plus what only this node knows about its account's rate limit.
    ///
    /// **Every entry point that takes a `Room` goes through here**, which is the point. The limit
    /// was originally passed in by each caller, and the fleet-of-one submit path handed
    /// `Capacity` straight in — `From<Capacity>` fills the field with `None`, so the gate simply
    /// did not exist on the path most of this project's testing uses. Measured: a node whose
    /// account was rate-limited for another 76 seconds started the next run immediately.
    ///
    /// That is the four-call-sites mistake this codebase has now made in five different words, so
    /// the fix is not a fifth call site. It is also the right *owner*: the limit is an observation
    /// this node made from its own agent's output, gossiped nowhere (ADR-0029), so the node is the
    /// only thing that can supply it and a caller doing so was always a caller repeating us.
    fn room(&self, room: impl Into<offload_core::Room>) -> offload_core::Room {
        let mut room = room.into();
        room.rate_limited_until = self.account_limited_until();
        room
    }

    /// Accept a run and start it here.
    ///
    /// The whole of placement on a node with no mesh, and the fast path on one with a mesh
    /// and nobody else awake.
    pub async fn submit(
        &self,
        req: crate::api::SubmitRequest,
        room: impl Into<offload_core::Room>,
    ) -> Result<RunId, SubmitError> {
        // Built before the capacity question rather than after, because since ADR-0013 the
        // answer depends on the run: what it costs the device is part of the request, and a
        // heavy run is refused by a machine that would have taken a light one.
        let run = self.build(req, &self.members_alone())?;
        // Plus whatever the machine's other fleets have promised: capacity is a property of the
        // device, and neither instance can see the other's runs. That, and everything else
        // after the build, is `submit_built`.
        self.submit_built(run, room).await
    }

    /// Turn a submission into a `Pending` run, deciding nothing about where it goes.
    ///
    /// Split out so the fleet can be asked in between (ADR-0006): with peers, the node that
    /// typed the command is the most likely host rather than the automatic one.
    /// Submit a task (ADR-0019). [`Self::submit`] for the cheap tier.
    ///
    /// The room check is the same one, deliberately: a task is cheaper than an agent run and
    /// it is not free, so it counts against the owner's ceiling like anything else. What it
    /// does *not* count against is an agent account — `agent_kind()` is `None` for a task and
    /// the account it shares is nobody's, which is a real zero rather than an unknown one.
    pub async fn submit_task(
        &self,
        req: crate::api::SubmitTaskRequest,
        room: impl Into<offload_core::Room>,
    ) -> Result<RunId, SubmitError> {
        let run = self.build_task(req, &self.members_alone())?;
        self.submit_built(run, room).await
    }

    /// Start a run that has already been built, if there is room here.
    ///
    /// The tail of both submissions above, and the whole of what a **schedule** needs: an
    /// occurrence arrives as a finished `Run` — its spec was written when the schedule was
    /// created and its id is derived from the tick (ADR-0056) — so there is nothing left to
    /// build, and everything after the build is the same for all three.
    ///
    /// Extracted rather than copied for the reason `place` is one function: this is where the
    /// owner's ceiling is charged, and a third path that forgot it would be a device quietly
    /// running past a cap its owner set.
    pub async fn submit_built(
        &self,
        run: Run,
        room: impl Into<offload_core::Room>,
    ) -> Result<RunId, SubmitError> {
        // The agent's own ceiling, against what this node has **committed** — the count a
        // submission is refused on, where a start asks what is going. This path is the whole of
        // placement on a node with no mesh, so on a fleet of one the ceiling was asked by
        // nothing at all.
        if let Some(refusal) = self.agent_full(&run, |kind| self.held_for_agent(kind)) {
            return Err(SubmitError::NoRoom(refusal));
        }
        self.room(room)
            .for_one_more(
                self.with_device(self.held()),
                run.spec
                    .agent_kind()
                    .map_or(0, |kind| self.held_for_agent(kind)),
                run.spec.demand,
            )
            .map_err(SubmitError::NoRoom)?;
        let id = run.id;
        self.start_run(run).await?;
        Ok(id)
    }

    /// Turn a task submission into a run.
    ///
    /// `Constraint::ServiceAuthenticated` with `Role::Execute` is the whole of the placement
    /// question: does anybody in this fleet have a *working* program for this service
    /// (ADR-0019 §1)? It is asked at the keyboard, so a service nobody can run is refused while
    /// the operator is still there (ADR-0014), and it does not pin placement to a device.
    ///
    /// **Not `HasService`,** which was here first and asks only whether somebody *nominated*
    /// one. A `[[tasks]]` block pointing at a path that does not exist satisfies that, and the
    /// probe already knows better — `task_capabilities` sets `authenticated` from whether the
    /// program is there, on every probe. Measured on one daemon with such a block: the run was
    /// accepted, placed here, and failed a millisecond later, which is the sentence ADR-0014
    /// exists to prevent and which the no-cluster arm had already been fixed for one cause over.
    /// With more than one node it is worse than a wasted run: the node that cannot do the work
    /// bids against the node that can.
    ///
    /// `Restartability::Idempotent` rather than `Resumable`, and not because the submitter may
    /// not say otherwise — `RunSpec::check` refuses that pairing — but because it is the true
    /// answer: a task that has to move starts again from the beginning, which is the only
    /// thing starting a program again can mean.
    pub fn build_task(
        &self,
        req: crate::api::SubmitTaskRequest,
        members: &[(NodeId, String)],
    ) -> Result<Run, SubmitError> {
        let crate::api::SubmitTaskRequest {
            service,
            args,
            queue,
            deadline,
            demand,
            notify,
            notices,
            origin,
            require,
            prefer,
            hold_until,
        } = req;

        let spec = RunSpec {
            work: offload_core::Work::Task(offload_core::TaskWork {
                service: service.clone(),
                args,
            }),
            constraint: Constraint::ServiceAuthenticated {
                service,
                role: Some(offload_core::Role::Execute),
            }
            .and(require.resolve(self.node_id, members)?),
            restartability: Restartability::Idempotent,
            priority: 0,
            queue,
            deadline: deadline.map(Millis),
            demand,
            notify,
            notices,
            resources: Vec::new(),
            prefer: prefer.resolve(self.node_id, members)?,
            hold_until: hold_until.map(Millis),
            parent: None,
        };
        // Asked here, where every submission passes, rather than at the CLI: a rule stored
        // months ago is fired by a daemon that never saw that parser.
        spec.check()?;

        // **Not saved here**, which is what `build` does and for a reason a walk found: a
        // submission the round refuses is one the operator was told about, and a row written
        // before the round leaves a `pending` task behind — visible in `offload ps`,
        // gossiped, and contradicting the CLI's own "(use `--queue` to leave it pending
        // anyway)" one line above it. `place` records what it decides to keep: `take_run` for
        // a run this node accepted, `record_run` for a queued one.
        // Stamped here for `build`'s reason: this is the one place a task's `Run` is made from a
        // submission, so ADR-0024's fact cannot be forgotten on the path that matters — a rule
        // firing one, which no peer can reconstruct.
        Ok(Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec,
            self.node_id,
            now(),
        )
        .started_by(origin))
    }

    /// This node as the only member it can name — what `here` and `node=` resolve against on a
    /// node with no fleet in view. The configured name, which is the only one it has there.
    #[must_use]
    pub fn members_alone(&self) -> Vec<(NodeId, String)> {
        vec![(self.node_id, self.config.name.clone())]
    }

    /// `members` is every node `node=<name>` may resolve to, this one included (ADR-0063 §3).
    pub fn build(
        &self,
        req: crate::api::SubmitRequest,
        members: &[(NodeId, String)],
    ) -> Result<Run, SubmitError> {
        let crate::api::SubmitRequest {
            repo,
            prompt,
            model,
            permission,
            git_ref,
            allow,
            queue,
            deadline,
            demand,
            notify,
            ask,
            resources,
            notices,
            max_turns,
            origin,
            require,
            prefer,
            hold_until,
        } = req;
        let permission = permission.unwrap_or(self.config.agent.default_permission_mode);

        // ADR-0008: refuse in the first second rather than let it fail forty turns later — but
        // only while it is still true. A run that may stop and ask has somebody to answer for
        // every gate `Ask` puts up, including the edits, so the refusal lifts exactly when the
        // approval channel covers the mode (ADR-0017).
        if permission == PermissionMode::Ask && !ask.enabled() {
            return Err(SubmitError::AskIsUnanswerable);
        }

        // Node baseline plus whatever this submission asked for. The repo's own layer is
        // added later, once its worktree exists to read `.offload.toml` from.
        let requested = ToolAllowlist::parse(&allow)?;
        let allow = self
            .config
            .node_allowlist()
            .map_err(|e| SubmitError::Agent(e.to_string()))?
            .merged_with(&requested);

        let run_id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let branch = offload_workspace::run_branch(run_id);

        let spec = RunSpec {
            work: Work::Agent(AgentWork {
                agent: AgentKind::ClaudeCode,
                model: model
                    .clone()
                    .or_else(|| self.config.agent.default_model.clone()),
                prompt: prompt.clone(),
                workspace: WorkspaceSpec {
                    repo: repo.clone(),
                    // Measured here rather than taken from the submitter, because the blob is
                    // already in this node's store by the time a run naming it is submitted
                    // (`StoreArchive` happens first, deliberately) — so the size is a fact this
                    // node can read rather than a number to be trusted, which is the same
                    // posture as hashing the bytes instead of believing the digest.
                    archive_bytes: offload_core::RepoSource::parse(&repo)
                        .archive()
                        .and_then(|hash| self.store.blob_size(hash)),
                    git_ref: git_ref.clone(),
                    branch: Some(branch.clone()),
                },
                permission_mode: permission,
                allow,
                max_turns,
                ask,
            }),
            // A grant is deliberately **not** a placement constraint any more (ADR-0011). It
            // was one for exactly as long as the ADR's own justification held — "until a
            // resource can be reached across the mesh, only its holder can honour a grant" —
            // and a proxied call is what retires it. Keeping it would defeat the case the ADR
            // was written for: the phone holds the mailbox and hosts nothing, so `--use email`
            // would pin the run to a device that cannot run it.
            //
            // What replaces it is a check at submission rather than a constraint at placement,
            // which is ADR-0014's argument in the place it belongs: a service *nothing* in the
            // fleet offers is refused while the operator is still at the keyboard, and a
            // service somebody offers is reached from wherever the run lands.
            constraint: Constraint::agent_ready(AgentKind::ClaudeCode, None)
                .and(require.resolve(self.node_id, members)?),
            restartability: Restartability::Resumable,
            priority: 0,
            queue,
            deadline: deadline.map(Millis),
            demand,
            notify,
            notices,
            resources,
            prefer: prefer.resolve(self.node_id, members)?,
            hold_until: hold_until.map(Millis),
            parent: None,
        };
        // The agent tier had no call here, because nothing in `SpecProblem` could be true of
        // an agent run until a hold could be typed.
        spec.check()?;

        // Stamped here rather than at the caller, because `build` is the one place a `Run` is
        // made from a submission and ADR-0024's whole value is that the fact cannot be forgotten
        // on the path that matters (a rule's, which no peer can reconstruct).
        Ok(Run::new(run_id, spec, self.node_id, now()).started_by(origin))
    }

    /// This node's name as the fleet sees it — its certificate's, falling back to the configured
    /// one — for a sentence another machine will print (`server::fleet_name`'s rule).
    pub fn fleet_name(&self) -> String {
        crate::fleet::display_name(&self.config.state_dir, &self.config.name)
    }

    /// Build a continuation of a finished run: a **new** run, starting from the parent's work
    /// and handed its closing message and transcript (ADR-0064).
    ///
    /// Asked of the node where the parent's last leg ran, because that is where its worktree
    /// is: the base is captured from it here — read, never written — and becomes the new run's
    /// starting checkpoint, so from this point it travels like any checkpoint does. A parent
    /// whose last leg ran elsewhere is refused with the sentence that says so rather than
    /// captured from a stale checkout, which would be the wrong base with nothing to tell.
    pub async fn build_continuation(
        &self,
        req: crate::api::ContinueRequest,
        members: &[(NodeId, String)],
    ) -> Result<Run, SubmitError> {
        let parent_id = self.resolve(&req.run)?;
        let parent = self
            .store
            .load_run(parent_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(req.run.clone()))?;
        let base = self.continuation_base(parent_id, req.mode, None).await?;
        self.continuation_from(req, &parent, base, members)
    }

    /// The half of a continuation only the parent's own node can build: the base, captured from
    /// the parent's worktree, and what the parent left behind (ADR-0064 §2).
    ///
    /// Asked here for a continuation typed here, and by a peer through
    /// `Host::continuation_base` for one typed there. The parent's **state** is checked here
    /// and not by the asker, because this is the node that finished it: a peer's gossiped copy
    /// can still say `running` a second after the run completed, and refusing on that would be
    /// a stale record answering for a fresh one.
    ///
    /// `asked_by_peer` carries this node's fleet name when a peer asked, because the refusals
    /// are about *this machine* and are read on the other one: "no longer on this node", printed
    /// at the asker's keyboard, names the machine the person is sitting at — the wrong one.
    pub async fn continuation_base(
        &self,
        parent_id: RunId,
        mode: offload_core::ContinueMode,
        asked_by_peer: Option<&str>,
    ) -> Result<offload_core::ContinuationBase, SubmitError> {
        let on = asked_by_peer.unwrap_or("this node");
        let parent = self
            .store
            .load_run(parent_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(parent_id.to_string()))?;
        let refuse = |reason: String| continuation_refused(parent_id, reason);
        match &parent.state {
            RunState::Completed { .. } | RunState::Cancelled { .. } => {}
            // A failed run reopens (ADR-0013), and continuing it as well would put two lines of
            // work on one base — one resumed by the recovery tick, one continued by a person.
            RunState::Failed { .. } => {
                return Err(refuse(format!(
                    "it failed, and a failed run is picked up where it stopped rather than \
                     continued — `offload resume {parent_id}`"
                )))
            }
            other => {
                return Err(refuse(format!(
                    "it is still {} — a run is continued once it has finished",
                    other.name()
                )))
            }
        }
        continued_work(&parent)?;

        // What this node's own log says about the parent's last leg: whether it ended here, the
        // agent session it ran as, the base its worktree was made from, and its closing words.
        let events = self.store.events_since::<LogEvent>(parent_id, 0)?;
        let mut ended_here = false;
        let (mut session, mut version, mut base, mut closing) = (None, None, None, None);
        for (_, event) in &events {
            match &event.kind {
                LogKind::AgentStarted {
                    session: s,
                    version: v,
                    ..
                } => {
                    session = Some(s.clone());
                    version = Some(v.clone());
                }
                LogKind::WorkspaceReady { base: b, .. } => base = Some(b.clone()),
                LogKind::Finished { result, .. } => {
                    ended_here = true;
                    closing.clone_from(result);
                }
                LogKind::Cancelled { .. } => ended_here = true,
                _ => {}
            }
        }
        if !ended_here {
            return Err(refuse(match asked_by_peer {
                None => "its last leg did not run on this node, so its workspace is not here — \
                         continue it from the node that finished it"
                    .to_string(),
                // The asker routed on `RunProgress::by`, and this node's own log disagrees: the
                // log is the authority on what ran here.
                Some(me) => {
                    format!("its last leg did not run on {me}, so its workspace is not there")
                }
            }));
        }
        let base = parent
            .checkpoint
            .as_ref()
            .map(|c| c.base_commit.clone())
            .or(base)
            .ok_or_else(|| refuse("its log records no workspace to start from".to_string()))?;
        let worktree = self
            .workspaces
            .finished_worktree(parent_id, &base)
            .await?
            .ok_or_else(|| {
                refuse(format!(
                    "its worktree is no longer on {on} — `offload rm` or the checkout sweep has \
                     taken it, and its work is on its branch only"
                ))
            })?;

        // The base, read-only on the parent's checkout: nothing is committed into it, because
        // that worktree is what a person may still be reading. `capture` touches only the index,
        // and reverts it.
        let capture =
            offload_workspace::checkpoint::capture(&worktree, &self.config.untracked_policy())
                .await?;
        let base_summary = capture.summary();

        // The agent's own file first: it is the whole conversation, where the last checkpoint's
        // copy stops at the last boundary it was taken at. The checkpoint's is the fallback.
        let transcript_bytes = session
            .as_deref()
            .and_then(|s| transcript::find(&self.home, &worktree.path, s).ok())
            .and_then(|path| std::fs::read(path).ok())
            .or_else(|| {
                parent
                    .checkpoint
                    .as_ref()
                    .filter(|c| self.store.has_blob(c.transcript))
                    .and_then(|c| self.store.get_blob(c.transcript).ok())
            });
        if mode == offload_core::ContinueMode::Session
            && (transcript_bytes.is_none() || session.is_none())
        {
            // Refused, never downgraded: this mode was asked for by name.
            return Err(refuse(format!(
                "`--session` resumes the parent's conversation, and its transcript cannot be \
                 found on {on} — continue it without `--session` to hand over its closing \
                 message and workspace instead"
            )));
        }

        let store = self.store.clone();
        let (bundle_bytes, patch_bytes) = (capture.bundle, capture.patch);
        let transcript_for_store = transcript_bytes.clone();
        let (transcript, checkpoint_transcript, bundle, patch) =
            tokio::task::spawn_blocking(move || {
                let transcript = transcript_for_store
                    .as_deref()
                    .map(|t| store.put_blob(t))
                    .transpose()?;
                // A checkpoint names a transcript blob by type. For a handoff with none to hand
                // over it is an empty one, which nothing reads: the base has no session, so it is
                // never resumed into.
                let checkpoint_transcript = match transcript {
                    Some(hash) => hash,
                    None => store.put_blob(&[])?,
                };
                let bundle = bundle_bytes.map(|b| store.put_blob(&b)).transpose()?;
                let patch = patch_bytes.map(|p| store.put_blob(&p)).transpose()?;
                Ok::<_, offload_store::StoreError>((
                    transcript,
                    checkpoint_transcript,
                    bundle,
                    patch,
                ))
            })
            .await
            .map_err(|e| SubmitError::Agent(format!("blob write task failed: {e}")))??;

        let session_mode = mode == offload_core::ContinueMode::Session;
        let checkpoint = Checkpoint {
            replicas: std::collections::BTreeSet::new(),
            session_id: session.clone().filter(|_| session_mode),
            transcript: checkpoint_transcript,
            bundle,
            patch,
            base_commit: worktree.base_commit.clone(),
            // Zero, which no real capture is: `Run::continuation_base` is how the start path
            // tells the parent's work from this run's own first checkpoint.
            turns: 0,
            taken_at: now(),
            // A fresh session reads nothing in the agent's private format, so a handoff asks no
            // version of anybody (ADR-0064 §3's portability). A fork is a resume and does.
            agent_version: if session_mode {
                version.unwrap_or_default()
            } else {
                "0".to_string()
            },
        };
        Ok(offload_core::ContinuationBase {
            checkpoint,
            transcript,
            closing_message: closing,
            summary: base_summary,
        })
    }

    /// Make a continuation from a base, on the node it was typed at (ADR-0064 §§1, 4).
    ///
    /// `parent` is whatever record this node has — its own row, or the fleet's gossiped copy of a
    /// run that never came here — and only its spec is read, which is immutable where it matters:
    /// the agent, the repository and the prompt. The state was checked where the base was built.
    pub fn continuation_from(
        &self,
        req: crate::api::ContinueRequest,
        parent: &Run,
        base: offload_core::ContinuationBase,
        members: &[(NodeId, String)],
    ) -> Result<Run, SubmitError> {
        let parent_id = parent.id;
        let work = continued_work(parent)?;
        let run_id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let requested = ToolAllowlist::parse(&req.allow)?;
        if work.permission_mode == PermissionMode::Ask && !req.ask.enabled() {
            return Err(SubmitError::AskIsUnanswerable);
        }

        let mut spec = parent.spec.clone();
        if let Some(agent) = spec.agent_mut() {
            agent.prompt.clone_from(&req.prompt);
            agent.workspace.branch = Some(offload_workspace::run_branch(run_id));
            agent.allow = agent.allow.merged_with(&requested);
            // This run's own budgets, counted afresh (§4).
            agent.max_turns = req.max_turns;
            agent.ask = req.ask;
        }
        // The parent's were about the parent (§4).
        spec.deadline = req.deadline.map(Millis);
        spec.hold_until = req.hold_until.map(Millis);
        spec.queue = req.queue;
        spec.priority = 0;
        spec.restartability = Restartability::Resumable;
        if let Some(require) = req.require {
            spec.constraint = Constraint::agent_ready(work.agent.clone(), None)
                .and(require.resolve(self.node_id, members)?);
        }
        if let Some(prefer) = req.prefer {
            spec.prefer = prefer.resolve(self.node_id, members)?;
        }
        spec.parent = Some(offload_core::Continuation {
            run: parent_id,
            mode: req.mode,
            transcript: base.transcript,
            parent_prompt: work.prompt.clone(),
            closing_message: base.closing_message,
        });
        spec.check()?;

        // An operator's, whoever started the parent: a person continuing a rule-fired run makes
        // a run a person asked for, which is true (§4).
        let mut run =
            Run::new(run_id, spec, self.node_id, now()).started_by(offload_core::Origin::Operator);
        run.checkpoint = Some(base.checkpoint);
        tracing::info!(
            run_id = %run_id,
            parent = %parent_id,
            mode = %req.mode,
            base = %base.summary,
            "continuation built"
        );
        Ok(run)
    }

    /// Take a granted run: start it now if there is room here, and **hold it** if there is not.
    ///
    /// ADR-0006's accepting-without-starting. A node that is full is not refusing — its queue
    /// empties by itself — and declining would leave the run `Pending` with nobody committed
    /// to it, which is the outcome the overnight case dies of. So the run stays `Assigned`
    /// here, holding the epoch and the lease, until [`Self::start_held_runs`] finds room.
    ///
    /// What makes this a commitment rather than a promise: the fleet can see who holds it,
    /// and if this node dies the lease expires and the ordinary orphan path takes it back.
    ///
    /// `capacity` is passed in rather than read here for the same reason `submit` takes it: the
    /// numbers that matter are the ones this node *gossiped*, so that what it bid with and what
    /// it admits with cannot drift apart.
    pub async fn take_run(
        &self,
        run: Run,
        room: impl Into<offload_core::Room>,
    ) -> Result<(), SubmitError> {
        let room = self.room(room);
        // Agents already going here, not counting this one — it is being granted to us right
        // now, and whether it arrived a moment ago must not change the answer.
        let running = self.started_excluding(run.id);
        // Anything already waiting goes first, even if there is room this instant: which of
        // two commitments starts is an ordering question, and it is answered in exactly one
        // place (`held_but_not_started`). Deciding it here as well would let a run that has
        // just arrived overtake one that has been waiting since midnight for the sole reason
        // that this is the code path it happened to arrive on.
        let queued = self
            .held_but_not_started()
            .iter()
            .any(|held| held.id != run.id);
        // The same question `start_held_runs` asks, through the same function. It was a second
        // copy of this arithmetic, and a second copy is how a clause gets added to one start path
        // and missed by the other — which is exactly what happened to the agent's own ceiling.
        // `running` is still computed above, for the log line below.
        let no_room = self.start_refusal(&run, room.clone());
        if no_room.is_none() && !queued {
            return self.start_run(run).await;
        }

        // Through the same transition the other branch takes, and for the same reason: this is
        // a whole-row write of a copy that crossed a network, so what it must not do is put
        // back a run this node has since finished, cancelled or given to somebody else.
        let run_id = run.id;
        let (run, epoch) = self.hold_here(run, "accepted")?;
        // A commitment is an assignment: ADR-0006's accept-without-starting means this node is
        // holding the run under a lease and an epoch, whether or not an agent exists yet, so the
        // audit trail records what was *promised* and not only what began. `start_run` records
        // the other branch, above.
        self.note_accepted(run_id, epoch);
        // The *reason*, where there is one, and not the generic sentence. This is the summary
        // beside the run in `offload ps` and it **gossips**, so "waiting for a slot" was the whole
        // fleet's answer about a run held by the account's rate limit on a machine with four free
        // slots — measured, on an idle laptop. Short and tenseless, not `describe_refusal`'s
        // sentence: see `summarise_refusal`. The generic wording stays for the case it was written
        // for — `queued` with room to spare, which is a run waiting behind another and nothing
        // else.
        match &no_room {
            Some(refusal) => self.note_workspace(run_id, &summarise_refusal(refusal)),
            None => self.note_workspace(run_id, "waiting for a slot"),
        }
        tracing::info!(
            run_id = %run_id,
            epoch = run.epoch.0,
            running = running.runs,
            shares = running.shares,
            demand = %run.spec.demand,
            budget = room.capacity.budget,
            max = room.capacity.max_runs,
            // Which of the two ceilings it is waiting on, because "the machine is free and
            // the run is not starting" is otherwise an unanswerable question.
            account_limit = room.account.as_ref().map(|a| a.limit),
            account_elsewhere = room.account.as_ref().map(|a| a.elsewhere),
            account_here = room
                .account
                .as_ref()
                .map(|_| {
                    run.spec
                        .agent_kind()
                        .map_or(0, |kind| self.started_excluding_on_agent(run_id, kind))
                }),
            // The refusal itself, so the log and the gossiped summary say the same thing. The
            // three fields above are the account *ceiling*'s arithmetic and are silent about a
            // rate limit, which is the reason a run is held on a machine with slots free.
            reason = no_room.as_ref().map(|r| r.to_string()),
            "accepted; holding it until it can start"
        );
        Ok(())
    }

    /// Why a run this node holds has not started yet, if it has not.
    ///
    /// **The same question [`Self::start_held_runs`] asks, and it is asked through this so the two
    /// cannot answer differently.** `offload explain` is the command whose whole job is "why is my
    /// run not running", and for a run in `Assigned` it said nothing about it at all — the state
    /// line gave the lease's remaining time and the canvass, which asks nodes about *taking* the
    /// run, answered "already holds this run". Which is true, and is not the question.
    ///
    /// Invisible while every reason was seconds long: a slot frees, a budget drains. ADR-0029 made
    /// one of them **hours** long, and then the one sentence that would have explained it was
    /// printed by `offload run` at submission and nowhere afterwards.
    ///
    /// `None` means nothing is holding it back, which for a run still `Assigned` a tick later
    /// means it is about to start.
    #[must_use]
    pub fn start_refusal(
        &self,
        run: &Run,
        room: impl Into<offload_core::Room>,
    ) -> Option<offload_core::Refusal> {
        // The agent's own ceiling, against what is **going** here. See `agent_full`.
        if let Some(refusal) =
            self.agent_full(run, |kind| self.started_excluding_on_agent(run.id, kind))
        {
            return Some(refusal);
        }
        self.room(room)
            .for_one_more(
                self.with_device(self.started_excluding(run.id)),
                run.spec
                    .agent_kind()
                    .map_or(0, |kind| self.started_excluding_on_agent(run.id, kind)),
                run.spec.demand,
            )
            .err()
    }

    /// Start the runs this node accepted but had no room for, now that it might.
    ///
    /// Polled from the daemon's tick for the same reason everything else here is: it cannot
    /// miss a transition, and a run finishing is not an event this node is the only witness
    /// to — a migration away frees a slot too.
    ///
    /// Returns the runs it started, for the caller to gossip.
    pub async fn start_held_runs(&self, room: impl Into<offload_core::Room>) -> Vec<Run> {
        let room = self.room(room);
        let mut started = Vec::new();

        for run in self.held_but_not_started() {
            // Re-checked per run rather than once: starting one fills the slot that the next
            // one would otherwise be told about. `break` rather than `continue`, still: the
            // order is the urgency order, and skipping past a heavy run to start a light one
            // behind it would quietly reorder the queue by cost.
            if let Some(no_room) = self.start_refusal(&run, room.clone()) {
                tracing::trace!(run_id = %run.id, reason = %no_room, "still no room for it");
                break;
            }
            let run_id = run.id;
            tracing::info!(run_id = %run_id, "there is room now; starting the run held for it");
            match self.start_run(run.clone()).await {
                Ok(()) => {
                    if let Some(fresh) = self.run(run_id) {
                        started.push(fresh);
                    }
                }
                // Two facts, and they are not the same fact. **What went wrong** is the error;
                // **whether anything will try again** is the tick's own filter, which is why
                // `will_try_again` asks it rather than enumerating error variants — a run that
                // ended, one that moved to another node and one whose workspace could not be
                // built are three failures with two answers, and the enumeration is what put
                // "yet" in front of runs nothing would ever look at again.
                Err(e) => {
                    let again = self.will_try_again(run_id);
                    match (&e, again) {
                        // A cancel that landed while the tick worked through the run ahead of
                        // it. The cancel worked, so this is not a warning.
                        (SubmitError::Ended { state, .. }, _) => tracing::info!(
                            run_id = %run_id,
                            state,
                            "it ended while it waited, so it is not being started"
                        ),
                        // The snapshot is filtered on `live`, so reaching this means an agent
                        // for the run started between the filter and here — and this arm is
                        // deliberately loud, because that is a door nobody has named yet
                        // (`docs/pitfalls/fencing-and-epochs.md`). Not a warning about the run,
                        // which is running: a warning about the two callers that met.
                        // `register` has already said so, with both epochs. This arm exists
                        // for the half it cannot say — *which* caller met it — and to keep the
                        // generic arm below from printing "nothing here will come back to it"
                        // about a run that is running.
                        (SubmitError::AlreadyGoing { .. }, _) => tracing::info!(
                            run_id = %run_id,
                            "an agent for it was already going here, so the tick started none"
                        ),
                        // Left `Assigned` deliberately: the next tick tries again, and the lease
                        // is what stops that being for ever.
                        (_, true) => {
                            tracing::warn!(run_id = %run_id, error = %e, "could not start it yet");
                        }
                        (_, false) => tracing::warn!(
                            run_id = %run_id,
                            error = %e,
                            "could not start it, and nothing here will come back to it"
                        ),
                    }
                }
            }
        }
        started
    }

    /// How many clients are streaming this run's events, if it is live here at all.
    ///
    /// The only way attendance is ever known (ADR-0013): a subscriber is a client following the
    /// run, so counting them is the observation. `None` means this node is not running it and
    /// therefore cannot say — which is a different answer from "nobody is watching".
    #[must_use]
    pub fn watchers(&self, run: RunId) -> Option<u32> {
        let local = lock(&self.live)
            .get(&run)
            .and_then(|entry| entry.live.as_ref())
            .map(|live| u32::try_from(live.receiver_count()).unwrap_or(u32::MAX))?;

        // Plus the peers that have asked for this run's log lately. A remote follower polls, so
        // "lately" is what stands in for a connection being open: a couple of missed polls is a
        // slow network, and a follower that has gone away stops counting within `WATCHER_GRACE`.
        let mut watchers = lock(&self.watchers);
        watchers.retain(|_, seen| seen.elapsed() < WATCHER_GRACE);
        let remote = watchers.keys().filter(|(id, _)| *id == run).count();
        Some(local.saturating_add(u32::try_from(remote).unwrap_or(u32::MAX)))
    }

    /// A page of a run's event log, and whether this node has more to come.
    ///
    /// The `done` half is the point: an empty page means nothing new *yet*, and a run between
    /// turns is quiet for minutes without being over. So a follower is told when to stop asking
    /// rather than left to guess — and what it is told is about *this leg*, since a run that has
    /// moved has its log split across the machines that ran it.
    pub fn events_after(
        &self,
        run: RunId,
        after: u64,
        limit: u32,
    ) -> Result<(Vec<offload_proto::cluster::SeqEvent>, bool), SubmitError> {
        let from = i64::try_from(after).unwrap_or(i64::MAX);
        let mut events: Vec<offload_proto::cluster::SeqEvent> = self
            .store
            .events_since::<LogEvent>(run, from)?
            .into_iter()
            .map(|(seq, event)| offload_proto::cluster::SeqEvent {
                seq: u64::try_from(seq).unwrap_or(0),
                event,
            })
            .collect();

        let capped = events.len() > limit as usize;
        if capped {
            events.truncate(limit as usize);
        }
        // More to fetch right now is not the same as more to come later, and only the second
        // one ends a follower's loop: this leg is over when its last event says so, or when the
        // run is no longer live here at all (a daemon that restarted mid-run has no agent to
        // produce anything, and its `Failed` record is the honest end of its log).
        let ended = events.last().is_some_and(|last| last.event.is_terminal())
            || (!capped
                && !self.stream_open(run)
                && self
                    .run(run)
                    .is_none_or(|r| r.state.is_terminal() || r.holder() != Some(self.node_id)));
        Ok((events, ended))
    }

    /// Note that a peer is following one of this node's runs.
    ///
    /// Called when it asks for the log rather than when it says it is watching, because asking
    /// is the only claim here that cannot be stale (ADR-0013).
    pub fn note_remote_watcher(&self, run: RunId, peer: NodeId) {
        lock(&self.watchers).insert((run, peer), std::time::Instant::now());
    }

    /// Note that a run has just failed here, and who was looking at the time.
    ///
    /// Sampled *now*, at the moment the failure is written, and not at the moment the decision
    /// is taken: a client following a run stops following when the stream ends, and a failure
    /// ends the stream. Asking a tick later would find nobody watching every single time.
    ///
    /// The resume count survives a second failure of the same run, because that is the loop this
    /// is meant to notice; it is a fresh entry — with a fresh budget — only after somebody has
    /// intervened, which is what removing the entry on escalation means.
    fn note_failure(&self, run: RunId) {
        let attendance = offload_core::Attendance::watching(self.watchers(run).unwrap_or(0));
        self.note_failure_seen_as(run, Some(attendance));
        tracing::info!(run_id = %run, %attendance, "run failed; noted who was watching");
    }

    /// The same note, for a failure nobody observed the attendance of.
    ///
    /// Used by the startup recovery path: those runs failed while this process did not exist, so
    /// the honest answer is that it cannot tell — and the decision that follows is to leave them
    /// for a person. Deliberately *not* "nobody was watching, so resume them all": a daemon in a
    /// crash loop would restart every run on every start, with no memory of having done it, and
    /// the bill for that arrives before anybody notices.
    fn note_failure_seen_as(&self, run: RunId, attendance: Option<offload_core::Attendance>) {
        let mut recovery = lock(&self.recovery);
        let entry = recovery.entry(run).or_insert(Recovering {
            attendance,
            resumes: 0,
            decided: None,
        });
        entry.attendance = attendance;
        // A fresh failure is a fresh question, so any earlier decision to leave it alone is not
        // an answer to it. The resume *count* survives on purpose — that is the loop this exists
        // to notice — and only a person intervening clears that (`Supervisor::resume`).
        entry.decided = None;
    }

    /// Say once, in the run's own log, that it is not going to make its deadline.
    ///
    /// ADR-0013's two reporting rows end here: a run nobody will take, and a run whose rate
    /// limit lifts after it is due. Both used to be silent — one retried for ever, the other
    /// recorded as a fact with no consequence — and silence is the wrong answer for the same
    /// reason in both cases: the fleet has established something only a person can act on, and
    /// no other part of this system is going to raise it.
    ///
    /// Four decisions worth keeping:
    ///
    /// * **Only a stated deadline reaches here.** [`Run::prospect_at`] is where that lives; the
    ///   caller passes its verdict in. An unspecified deadline means the moment of submission,
    ///   so a rule that skipped it would announce every ordinary run in the fleet.
    /// * **Once per deadline**, which is the latch on `overdue`. An operator who moves the
    ///   deadline gets a fresh answer, because that is the one intervention that makes the old
    ///   announcement wrong rather than repetitive.
    /// * **It changes nothing about the run.** A deadline decides when we give up, never what
    ///   we may do (ADR-0013) — so a run that has missed one keeps being offered, keeps its
    ///   checkpoint, and keeps its lease. This is a sentence, not a decision.
    /// * **The log, not just a `WARN`.** The log is what `offload logs` reads from any node and
    ///   what ADR-0010's delivery plane will fan out from. A tracing line reaches whoever is
    ///   tailing the daemon on the machine that happened to notice, which for a run that broke
    ///   at 03:00 is nobody.
    ///
    /// Returns whether it said anything, which is what the caller logs.
    pub fn note_overdue(&self, run: &Run, by: Millis, waiting: Waiting) -> bool {
        let due = run.due_at();
        {
            let mut said = lock(&self.overdue);
            if said.get(&run.id) == Some(&due) {
                return false;
            }
            said.insert(run.id, due);
        }
        tracing::warn!(
            run_id = %run.id,
            overdue_by = %by,
            waiting = %waiting,
            "this run will not make its deadline"
        );
        self.append(
            run.id,
            LogKind::Overdue {
                by_ms: by.0,
                waiting,
            },
        );
        true
    }

    /// Stop intending to resume a run, and forget what this node had decided about retrying it.
    ///
    /// Two callers, and they mean the same thing from opposite ends: **a firing has superseded the
    /// occurrence** (ADR-0027), and **a person has taken the run over** (`offload resume`). The
    /// second one is where the "fresh budget" rule lives now — it used to be a side effect of the
    /// escalate branch deleting the entry, so it applied only to a run that had already been given
    /// up on, and a person resuming one still in backoff inherited its count.
    ///
    /// **Not the auto-resume path**, which is the one caller that must *not* clear it: that path
    /// writes the count it is spending and would erase it, and an auto-resume with no memory of
    /// having happened is the spin loop `max_resumes` exists to stop.
    ///
    /// The ADR-0027 half, and the reason it exists is that two mechanisms could not see each
    /// other. A
    /// rule fires **one occurrence at a time** (ADR-0020 §3) — an event arriving while the last
    /// run is going is dropped, because a watcher's value is the current state — and auto-resume
    /// picked the *previous* occurrence back up regardless, because [`Self::recover_failed_runs`]
    /// walks a watch list that knows nothing about rules and `rule_run_in_flight` reads
    /// `rules.last_run`, which by then names the newer occurrence. Measured: 27 firings, 39
    /// resumes, two agents from a rule reporting one, each resumed leg working on an event
    /// half a minute old.
    ///
    /// Returns whether it withdrew anything, and the caller says so — "why did my occurrence stop
    /// at turn 3 and never come back" has a one-line answer and nowhere else to read it.
    ///
    /// **Not** a way to stop a *running* agent: this is the intention to start one later, and the
    /// entry it removes is the same one [`offload_core::Recovery::Escalate`] removes when the
    /// decision is to leave a run for a person. What that means for the caller is that a run
    /// already resumed is unaffected — it is running, and the rule's in-flight test is what
    /// covers it.
    pub fn stop_recovering(&self, run: RunId) -> bool {
        lock(&self.recovery).remove(&run).is_some()
    }

    /// Pick failed runs back up, or leave them in front of the person who was watching.
    ///
    /// ADR-0013's third axis, and the only decision in this system that turns on attendance.
    /// Two things worth knowing before changing it:
    ///
    /// * **It is the holder's decision, not the arbiter's.** A `Failed` run is terminal as far
    ///   as `offload_core::supervise` is concerned, and rightly: the node whose agent stopped is
    ///   the node holding the checkpoint, the warm worktree, *and* the stream somebody was
    ///   watching. Nothing about it has to travel, so nothing about it is gossiped.
    /// * **A resume this node could not even start still counts.** Otherwise a run that cannot
    ///   be resumed here for a reason of its own — no room, a worktree that will not build —
    ///   retries at tick rate for ever. Counting it means the run lands in front of a person
    ///   after `max_resumes` rather than churning quietly.
    ///
    /// `hosting` is the owner's standing answer, asked of the **live** capabilities by the
    /// caller rather than read from anything here (ADR-0049). A parameter rather than a field on
    /// the supervisor, and rather than a `Config` lookup in this function, because it changes
    /// under a running daemon — a battery drains, a link becomes metered — so it has to be
    /// answered per pass, from the probe the rest of the daemon is deciding with (ADR-0048).
    ///
    /// Returns what it did: the runs it restarted here, and how many it gave back to the fleet.
    pub async fn recover_failed_runs(
        &self,
        capacity: Capacity,
        policy: &offload_core::RecoveryPolicy,
        // Asked per *run* rather than once for the pass, because since ADR-0019 §4 the owner's
        // standing answer depends on what is being asked about: a light watcher and an
        // expensive agent session can get different answers from one policy, and this pass
        // walks a list that may hold both. A closure rather than a `WorkPolicy` and a
        // `Capabilities` so the caller keeps the ADR-0048 rule — one read of the live handle,
        // captured once, for every run in the pass.
        hosting: &(dyn Fn(&Run) -> offload_core::Hosting + Send + Sync),
        // Whether this node knows of any other node at all. A parameter rather than something
        // read here for the reason `hosting` is one: the supervisor holds no cluster, and the
        // answer belongs to whoever does.
        alone: bool,
    ) -> Recovered {
        let watching: Vec<(RunId, Recovering)> = lock(&self.recovery)
            .iter()
            .map(|(id, state)| (*id, *state))
            .collect();
        let now = now();
        let mut out = Recovered::default();

        for (run_id, state) in watching {
            // Already answered, and the answer is remembered rather than repeated. This is what
            // used to be done by deleting the entry; the difference is that `offload explain` can
            // still say why.
            if state.decided.is_some() {
                continue;
            }
            let Some(run) = self.run(run_id) else {
                lock(&self.recovery).remove(&run_id);
                continue;
            };
            // This node's own count for the run, which is the one that has to be right here:
            // the numbers gossip, and a peer's copy of a run *this* node holds the failure of
            // is exactly the copy `absorb` refuses to believe.
            let turns = self.turns_taken(run_id);
            match offload_core::decide_recovery(
                &run,
                now,
                policy,
                offload_core::Circumstances {
                    attendance: state.attendance,
                    standing: self.standing_to_retry(&run),
                    // The path `stop_accepting` never reached. A drain refuses bids and refuses
                    // grants; this asks neither, and went on spawning agents on a drained node.
                    departing: self.is_draining(),
                    // The seventh door on this gate, and the last one that spawns an agent
                    // (ADR-0049). Measured: a node reporting `accepting no — network is metered
                    // and policy disallows it`, whose submission and resume doors both refused,
                    // logged `nobody was watching; resuming it` sixty seconds later and ran the
                    // agent to turn 14 on the link its owner is paying for.
                    hosting: hosting(&run),
                    // Handed in, because the supervisor has no view: whether this fleet has a
                    // second device is the cluster's fact, and a run released into a pool with
                    // no members is a run nothing offers again (ADR-0043's premise).
                    alone,
                    // Above `departing`, and it is checked here for the same reason that one is:
                    // nothing on this path asks the bid or the grant, which are the doors a
                    // revocation shuts everywhere else (ADR-0044).
                    revoked: self.is_revoked(),
                    turns,
                    resumes: state.resumes,
                },
            ) {
                // Not failed: either the resume took and it is running again — in which case the
                // count has to stay, or a second failure would arrive with a fresh budget — or
                // it is over and there is nothing left to remember.
                None => {
                    if run.state.is_terminal() {
                        lock(&self.recovery).remove(&run_id);
                    }
                }
                Some(offload_core::Recovery::Wait { until, resume }) => {
                    tracing::trace!(
                        run_id = %run_id,
                        in_ms = until.saturating_sub(now).0,
                        resume,
                        "waiting before picking it up again"
                    );
                }
                // Left `Failed`, and left alone: `offload resume` is a person's decision now.
                // Written down rather than deleted, so it is said once *and* can still be
                // answered for — `offload explain` had nothing to print here and fell back to
                // sampling the stream, which for a failed run is always "nobody is watching"
                // (ADR-0013's own module docs say so). A person resuming it clears the entry,
                // which is where the fresh budget comes from now.
                Some(offload_core::Recovery::Escalate(reason)) => {
                    tracing::info!(
                        run_id = %run_id,
                        %reason,
                        "leaving it failed for a person to look at"
                    );
                    if let Some(entry) = lock(&self.recovery).get_mut(&run_id) {
                        entry.decided = Some(reason);
                    }
                }
                // On the way out, and the fleet can carry on with it (ADR-0043). The effect is
                // a *transition*, not a start: the run leaves `Failed` for `Pending` with this
                // node named on it, and whoever arbitrates offers it — which is the one offerer
                // ADR-0042 went to some trouble to make it. This node bids for nothing, because
                // it is draining.
                //
                // Inside `update_run`, because the epoch bump is the fence: run on the copy read
                // above it would be the load-modify-save that put two agents in one worktree.
                Some(offload_core::Recovery::LetGo) => {
                    match self
                        .store
                        .update_run(run_id, |run| run.let_go(self.node_id, now))
                    {
                        Ok(Some(Ok(()))) => {
                            // The cause, not one of the two causes. `LetGo` had one when this
                            // line was written and has two now (ADR-0049), and a log saying
                            // "this node is leaving" about a laptop that is sitting right there
                            // on a metered link is the wrong cause confidently stated — the
                            // third place that sentence lives, and the one the ADR missed. Found
                            // by the walk it had said it could not do.
                            tracing::info!(
                                run_id = %run_id,
                                reason = if self.is_draining() {
                                    "this node is leaving"
                                } else {
                                    "this node's owner does not allow it to host runs"
                                },
                                "handing the failed run back to the fleet"
                            );
                            // The count was this node's memory of retrying it *here*, and here
                            // is over. Not the deletion `stop_recovering` is warned about: that
                            // one hands a fresh budget to the same machine, and this run is not
                            // coming back to this one — it has left `Failed` altogether, so
                            // nothing on this node will ask about it again.
                            lock(&self.recovery).remove(&run_id);
                            // Returned so the caller can put it in front of the fleet *now*.
                            // The periodic republish would get there on the next tick, which is
                            // fine for the door the daemon stays up on and is not available on
                            // the other one — see `hand_back_failed_runs`.
                            if let Some(fresh) = self.run(run_id) {
                                out.handed_back.push(fresh);
                            }
                        }
                        // It moved on underneath us — somebody resumed it, cancelled it, or a
                        // grant landed. The row is right and this pass is stale.
                        Ok(Some(Err(e))) => tracing::debug!(
                            run_id = %run_id, error = %e, "it was no longer ours to let go"
                        ),
                        Ok(None) => {
                            lock(&self.recovery).remove(&run_id);
                        }
                        Err(e) => tracing::warn!(
                            run_id = %run_id, error = %e, "could not hand the failed run back"
                        ),
                    }
                }
                Some(offload_core::Recovery::Resume { resume }) => {
                    if let Some(entry) = lock(&self.recovery).get_mut(&run_id) {
                        entry.resumes = resume;
                    }
                    // **Two tiers, two verbs, and the decision above says neither of them.**
                    // `Recovery::Resume` is the answer for both — the core decides whether to
                    // pick a run back up and has no business knowing what starting it means —
                    // and until ADR-0058 this arm had one mechanism, `resume`, which refuses a
                    // task on its fourth line. So the tick asked, was refused in microseconds,
                    // wrote "this attempt still counts", and spent the run's whole retry budget
                    // in ninety seconds without ever starting the program. A word an operator
                    // reads is part of it: the log said a shell script had no conversation.
                    let task = self
                        .run(run_id)
                        .is_some_and(|run| run.spec.work.kind() == offload_core::WorkKind::Task);
                    tracing::info!(
                        run_id = %run_id, resume,
                        "nobody was watching; {}",
                        if task { "restarting it" } else { "resuming it" }
                    );
                    let outcome = if task {
                        self.restart_task(run_id, capacity).await
                    } else {
                        self.resume(run_id, None, capacity).await
                    };
                    match outcome {
                        Ok(()) => {
                            if let Some(fresh) = self.run(run_id) {
                                out.resumed.push(fresh);
                            }
                        }
                        Err(e) => tracing::warn!(
                            run_id = %run_id, error = %e, resume,
                            "could not pick it back up; this attempt still counts"
                        ),
                    }
                }
            }
        }
        out
    }

    /// Give the fleet back every failed run this node would have picked up itself (ADR-0043).
    ///
    /// Called by [`crate::mesh::depart`], and it has to be called there rather than left to the
    /// recovery tick — which is what a departing node's tick already does, one tick later at
    /// most. **One tick later is not always available.** `Mesh::drain` returns immediately when
    /// this node holds no live run, and a node whose only run has *failed* holds none; on the
    /// signal path `main` then stops everything, so the tick that would have handed the run
    /// back never fires. That is the `SIGTERM` door, which is what closing a laptop looks like,
    /// and it is the door ADR-0041 was measured on and left open.
    ///
    /// **`Capacity::runs(0)` is not a placeholder.** Capacity is read in exactly one place here
    /// — the resume — and `decide_recovery` cannot return `Resume` for a departing node, so it
    /// is unreachable. Passing nought rather than the node's real figure makes that a second and
    /// independent guard: if the first one ever breaks, the resume is refused for want of room
    /// instead of starting an agent on a machine that is leaving.
    pub async fn hand_back_failed_runs(&self, alone: bool) -> Vec<Run> {
        debug_assert!(
            self.is_draining(),
            "the departing flag is what makes this pass safe, and `depart` sets it first"
        );
        // `Allowed`, and it is not a claim about the owner's policy: this pass runs with
        // `departing` set, which outranks it and produces the same `LetGo` either way. The only
        // place the two differ is the sentence when nobody else holds a copy, and there the
        // right one is the departing node's — it *is* leaving (ADR-0049).
        self.recover_failed_runs(
            Capacity::runs(0),
            &offload_core::RecoveryPolicy::default(),
            &|_| offload_core::Hosting::Allowed,
            alone,
        )
        .await
        .handed_back
    }

    /// What this node intends to do about a failed run, and the attendance that decided it.
    ///
    /// The pair `offload explain` needs and had neither of. It printed `attendance unattended —
    /// nobody is streaming it` about a run this node had left alone *because somebody was
    /// watching* — measured, one daemon, one second apart from the daemon's own
    /// `reason=somebody was watching it when it failed`. ADR-0013's module docs predicted exactly
    /// that: "sampling attendance a tick after the fact would find nobody watching every single
    /// time", which is why the observation is taken when the run fails and has to be *remembered*
    /// rather than re-taken.
    ///
    /// `None` means this node has no opinion to report: the run never failed here, or it failed
    /// while some earlier incarnation of this daemon was running. That second case is honest
    /// rather than empty — it is the same thing `Escalation::AttendanceUnknown` says, and a node
    /// that has forgotten must not answer as though it remembers.
    #[must_use]
    pub fn recovery_state(&self, run: RunId) -> Option<RecoveryState> {
        let state = *lock(&self.recovery).get(&run)?;
        Some(RecoveryState {
            observed: state.attendance,
            resumes: state.resumes,
            decided: state.decided,
        })
    }

    /// How many turns a run has taken, over its whole life rather than over this leg of it.
    ///
    /// This node's own copy, from the store, which is what every *decision* here reads:
    /// `RunProgress` gossips, and a merge can hand this node a peer's numbers for a run — so
    /// the one thing a limit must not be evaluated against is a figure that could arrive from
    /// somewhere else while the local agent is between turns. The pump asks the boundary
    /// directly for the same reason, and never this.
    ///
    /// Zero for a run this node has no numbers for, which is the safe direction: it under-counts
    /// rather than over-counts, so an unknown never *ends* a run.
    #[must_use]
    pub fn turns_taken(&self, run: RunId) -> u32 {
        self.store.load_stats(run).unwrap_or_default().turns
    }

    /// Whether this node may decide about retrying a run at all (ADR-0030).
    ///
    /// One question, asked of two facts that are already there. `Run::origin` **travels**
    /// (ADR-0024), so a node that has never heard of a rule can tell an occurrence from somebody's
    /// work; `runs.rule` is written by the firing and left alone by a gossip merge, so "tagged
    /// here" means "a rule on this machine fired it". An occurrence with no local tag is therefore
    /// somebody else's rule's, and retrying it is acting on events this node does not see.
    ///
    /// A store error reads as **ours**, which is the pre-ADR-0030 behaviour: the wrong direction
    /// costs a stale retry, and the other wrong direction would silently stop retrying the runs
    /// this axis was built for on any node whose store hiccuped.
    fn standing_to_retry(&self, run: &Run) -> offload_core::Standing {
        if run.origin != offload_core::Origin::Rule {
            return offload_core::Standing::Ours;
        }
        match self.store.is_local_occurrence(run.id) {
            Ok(true) => offload_core::Standing::Ours,
            Ok(false) => offload_core::Standing::MachineStartedElsewhere,
            Err(e) => {
                tracing::warn!(
                    run_id = %run.id, error = %e,
                    "could not tell whose rule started this run; treating it as ours"
                );
                offload_core::Standing::Ours
            }
        }
    }

    /// Give back the runs this node accepted but never started.
    ///
    /// A commitment to start something later (ADR-0006) is worth exactly as much as the node
    /// that made it, so a node on its way out hands it back to the pool: `Pending` again, at a
    /// fresh epoch, ready for the drain to offer it to somebody who will actually run it.
    ///
    /// `live` is the discriminator rather than the state, for the same reason it is in
    /// [`Self::held_but_not_started`]: `start_run` writes `Assigned` and then launches, so a
    /// run that started a moment ago looks identical to one that never will — and releasing
    /// that one would abandon a live agent.
    pub fn release_unstarted(&self, held: &[Run]) -> Vec<Run> {
        let mut released = Vec::new();
        for run in held {
            if !matches!(run.state, RunState::Assigned { .. })
                || run.holder() != Some(self.node_id)
                || Self::agent_here(&lock(&self.live), run.id)
            {
                continue;
            }
            // Released from the row, not from the copy the caller handed us: `held` was read
            // before this node decided to drain, and a commitment that has started since is one
            // this must not hand to somebody else. `release` fences, and the row is what it
            // fences against.
            let handed = self.store.update_run(run.id, |run| {
                let epoch = run.epoch;
                run.release(self.node_id, epoch, now())
                    .ok()
                    .map(|()| run.clone())
            });
            match handed {
                Ok(Some(Some(run))) => {
                    tracing::info!(run_id = %run.id, "handing back a run we had not started");
                    // The note `take_run` left is *why this node has not started it yet* — "at
                    // capacity: 1/1 runs", "waiting for a slot" — and this node has stopped
                    // intending to. Left in place it is `offload ps` naming a wait that ended,
                    // in the gossiped column, about a run nobody holds: measured on a released
                    // commitment sitting `pending` for two minutes on an idle node whose
                    // WORKSPACE column still read `waiting for a slot`. Cleared rather than
                    // reworded, because there is no workspace to summarise — nothing ran — and
                    // the state column already says `pending`. Here rather than in either
                    // caller: the drain and the late-commitment pass both let a run go, and a
                    // fact hoisted into "the caller" has as many owners as there are callers.
                    self.note_workspace(run.id, "");
                    released.push(run);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(run_id = %run.id, error = %e, "could not release it"),
            }
        }
        released
    }

    /// Renew the lease on every run this node is holding.
    ///
    /// A lease is a claim with a deadline, and until this existed nothing ever moved the
    /// deadline: a run granted by a peer carried the arbiter's 60-second lease for its whole
    /// life, so an arbiter watching a perfectly healthy holder saw the lease lapse and marked
    /// the run `Orphaned` a minute after granting it. The holder's next gossip reclaimed it —
    /// the merge rule is what stopped that being a migration — so the visible symptom was a
    /// run flapping between `running` and `orphaned` for ever, and the invisible one was a
    /// hold-down deciding about a node that had never gone anywhere.
    ///
    /// Accepting-without-starting makes this load-bearing rather than cosmetic: a held run's
    /// entire commitment *is* the lease.
    pub fn heartbeat(&self) {
        let me = self.node_id;
        let now = now();
        let mut held = Vec::new();
        for listed in self.store.list_runs(false).unwrap_or_default() {
            if listed.holder() != Some(me) {
                continue;
            }
            // Renewed against the row as it stands, never against the listing above it. A lease
            // is one field and `save_run` writes every field, so the copy this loop was handed
            // — read before the runs ahead of it in the list were even written — would put the
            // whole run back as it was a moment ago. What it undoes is whatever happened in
            // between: a checkpoint at a turn boundary, or the agent finishing. That last one
            // is the ugly shape, because renewing a run this loop has just resurrected keeps it
            // `Running` with no agent behind it *and* keeps anything from ever orphaning it.
            let renewed = self.store.update_run(listed.id, |run| {
                if run.holder() != Some(me) {
                    return None;
                }
                let epoch = run.epoch;
                match run.renew(me, epoch, now, LEASE_TTL) {
                    Ok(()) => {}
                    // `Pending` and terminal runs have no lease to renew, which is not a fault:
                    // a checkpointed run waiting for `offload resume` is holding nothing.
                    Err(e) => {
                        tracing::trace!(run_id = %run.id, error = %e, "nothing to renew");
                    }
                }
                Some(run.clone())
            });
            match renewed {
                Ok(Some(Some(run))) => held.push(run),
                // Gone, or somebody else's now — either way there is nothing here to renew.
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(run_id = %listed.id, error = %e, "could not renew a lease");
                }
            }
        }
        self.reconcile_reservations(&held);
    }

    /// Runs held here, in `Assigned`, with no agent behind them — most urgent first.
    ///
    /// `live` is the discriminator, not the state: `start_run` saves `Assigned` and launches,
    /// so a run that has just been started is briefly in the same state as one that is
    /// waiting. Reading the process table rather than the state is what keeps a poll from
    /// starting the same agent twice.
    ///
    /// The order is the only place in this system where one exists at all — nodes bid
    /// independently and there is no queue to serialise (ADR-0013). It is `urgency_order`:
    /// least slack first, priority breaking ties, so a batch submitted together comes out
    /// roughly in the order it went in and nothing waits behind a run that is due later.
    /// The runs this node has committed to and not started, most urgent first.
    ///
    /// Public because the *other* question about a commitment — whether it is still worth
    /// keeping — needs the fleet's view and therefore lives in the mesh, while what counts as
    /// a commitment is knowable only here (see the note in [`Self::release_unstarted`] about
    /// why `live` is the discriminator rather than the state).
    #[must_use]
    pub fn held_unstarted(&self) -> Vec<Run> {
        self.held_but_not_started()
    }

    /// The three things that make a run one this node's tick is going to start.
    ///
    /// Written once because two callers ask it and they must not disagree:
    /// [`Self::held_but_not_started`] *selects* on it, and [`Self::will_try_again`] reports on it.
    /// A log line that promises the next tick will try again is a claim about this predicate, and
    /// a second copy of it in a `match` on error variants is the shape `reports-and-cli` is about.
    ///
    /// Associated rather than a method, because the caller already holds the `live` lock.
    fn awaits_a_slot(me: NodeId, run: &Run, live: &BTreeMap<RunId, LiveRun>) -> bool {
        run.holder() == Some(me)
            && matches!(run.state, RunState::Assigned { .. })
            && !Self::agent_here(live, run.id)
    }

    /// Is there an agent for this run going **here, and now**?
    ///
    /// The cancel handle, and deliberately not the entry: a [`LiveRun`] outlives the leg that
    /// made it (see its doc), so *an entry exists* and *an agent is going* stopped being the
    /// same question the first time a run came back to a node that had already run a leg of it.
    /// Presence answered the second with the first, and the answers are opposite instructions.
    ///
    /// What that cost: a run granted back here while the node was full took `hold_here`, which
    /// does not touch `live` — so the previous leg's entry was still sitting there, and
    /// [`Self::awaits_a_slot`] read it as *already started*. The run stayed `Assigned` at a
    /// perfectly good lease, renewed for ever, and no tick ever came back to it. The same
    /// misreading made [`Self::started_excluding`] count it against the machine's own capacity,
    /// which is the deadlock `two_commitments_on_a_one_slot_node_do_not_block_each_other` exists
    /// to prevent, and made a drain leave it behind instead of handing it back.
    ///
    /// Associated for [`Self::awaits_a_slot`]'s reason: two of the three callers are already
    /// holding the lock.
    fn agent_here(live: &BTreeMap<RunId, LiveRun>, run: RunId) -> bool {
        live.get(&run).is_some_and(|leg| leg.cancel.is_some())
    }

    /// May this leg still append to the run's log?
    ///
    /// The other half of [`Self::agent_here`], and a *later* moment than it: `release` drops the
    /// cancel handle before the terminal write, and the run's last lines are appended after
    /// that. What ends the log is [`Self::close_stream`], at the end of `launch`, so the stream
    /// is what a follower has to ask about — asking for the agent would end it a few lines early.
    fn stream_open(&self, run: RunId) -> bool {
        lock(&self.live)
            .get(&run)
            .is_some_and(|leg| leg.live.is_some())
    }

    /// Will the tick come back to this run, or was that the end of it here?
    ///
    /// Asked *after* a start has failed, and the answer is not a property of the error: a run
    /// that ended, one that moved to another node, and one whose workspace could not be built
    /// are three failures with two answers, and enumerating them is how "could not start it yet"
    /// came to be printed about runs nothing would ever look at again. This asks the filter.
    ///
    /// Sound only because a failed [`Self::start_run`] takes its registration back — otherwise
    /// every failure would answer `false` by way of the `live` check, which is the wedge rather
    /// than the report.
    fn will_try_again(&self, run_id: RunId) -> bool {
        let live = lock(&self.live);
        self.store
            .load_run(run_id)
            .ok()
            .flatten()
            .is_some_and(|run| Self::awaits_a_slot(self.node_id, &run, &live))
    }

    fn held_but_not_started(&self) -> Vec<Run> {
        let live = lock(&self.live);
        let mut held: Vec<Run> = self
            .store
            .list_runs(false)
            .unwrap_or_default()
            .into_iter()
            .filter(|run| Self::awaits_a_slot(self.node_id, run, &live))
            .collect();
        let now = now();
        held.sort_by_key(|run| run.urgency_order(now));
        held
    }

    /// How many agents are actually going here, ignoring one run.
    ///
    /// **Capacity gates how many agents run, not how many runs are held.** Those are two
    /// questions with two answers, and answering the second with the first deadlocks a node
    /// that took ADR-0006 at its word: three runs granted to a one-slot machine leaves it, once
    /// the first finishes, holding two commitments — and each of them then reads the *other* as
    /// occupying the only slot, so neither ever starts and the leases renew for ever. Two
    /// submissions never showed it, because one commitment has nothing to be blocked by.
    ///
    /// Holding remains the right count for whether to accept *more* work
    /// ([`Self::held_count`], which is what bidding and `admits` read): a commitment is a
    /// promise about a slot, so a node that has made two of them is full even while idle.
    ///
    /// Live *or* `Running`, because neither alone is the truth for the whole of a run's start:
    /// `start_run` registers the agent before the state moves and a workspace can take a minute
    /// to clone, while the `live` map keeps its entry after the run has finished. Terminal runs
    /// are excluded by asking the store for the live ones only.
    /// Weighed as well as counted, for the same reason [`Self::held`] is: a budget and a count
    /// that disagree about which runs they describe produce a refusal nobody can reconstruct.
    /// [`Self::started_excluding`], narrowed to one agent — the count an *account's* ceiling is
    /// measured against.
    ///
    /// Excluding this run matters more here than it looks: a held run is `Assigned`, so counting
    /// it against the account would make every commitment the reason it could not start, and the
    /// run would wait for itself for ever. That is the same deadlock the machine's own capacity
    /// hit when it answered "may I start" with "what do I hold".
    fn started_excluding_on_agent(&self, run: RunId, agent: &offload_core::AgentKind) -> u32 {
        let live = lock(&self.live);
        let me = self.node_id;
        let mut going = 0u32;
        for r in self.store.list_runs(false).unwrap_or_default() {
            let mine = r.id != run && r.holder() == Some(me) && r.spec.agent_kind() == Some(agent);
            let started = Self::agent_here(&live, r.id)
                || matches!(
                    r.state,
                    RunState::Running { .. } | RunState::Checkpointing { .. }
                );
            if mine && started {
                going = going.saturating_add(1);
            }
        }
        going
    }

    fn started_excluding(&self, run: RunId) -> Occupancy {
        let live = lock(&self.live);
        let me = self.node_id;
        let mut going = Occupancy::default();
        for r in self.store.list_runs(false).unwrap_or_default() {
            let mine = r.id != run && r.holder() == Some(me);
            let started = Self::agent_here(&live, r.id)
                || matches!(
                    r.state,
                    RunState::Running { .. } | RunState::Checkpointing { .. }
                );
            if mine && started {
                going.runs = going.runs.saturating_add(1);
                going.shares = going.shares.saturating_add(r.spec.demand.shares());
            }
        }
        going
    }

    /// Make this node the run's holder — **deciding against the row, not against the copy the
    /// caller is holding.**
    ///
    /// Both entrances to holding a run come through here: starting it now, and ADR-0006's
    /// accepting-without-starting. Two copies of this would be two chances to disagree about
    /// what a stale copy means, and it is the write that decides, so it is the one that has to
    /// ask.
    ///
    /// **Read, decide and write under one lock**, like `Supervisor::resume`, and for the same
    /// reason: what this replaced did `assign` on the copy it was handed and then a whole-row
    /// `save_run`, which does not overrule what landed in between — it erases it. Two legs
    /// assigned from one loaded copy also compute the *same* epoch, and that is the one thing
    /// no later fence can separate.
    ///
    /// The handed copy is as old as whatever its caller did on the way here, and one caller
    /// does something slow. `start_held_runs` snapshots the runs it will start and works
    /// through them, so every copy after the first is as old as the run ahead of it took to
    /// start — and `ensure_blobs`, a network fetch of a transcript, is an `await` in the middle
    /// of that. Measured with a cancel landing in the window: the run came back **`Failed`**,
    /// having been restored to `Assigned`, started, and then failed by the agent that should
    /// never have run. `Failed` is a state auto-resume picks up, so the cancel did not merely
    /// fail to take — it left the run resumable.
    ///
    /// `action` is the verb for the refusal, because the two callers are refusing different
    /// things: a run this node cannot start, and a grant it cannot accept.
    fn hold_here(&self, run: Run, action: &'static str) -> Result<(Run, Epoch), SubmitError> {
        let run_id = run.id;
        self.store.upsert_run(run_id, |stored| {
            let mut run = match stored {
                // Never seen here: a submission, whose row's first existence is this write.
                // The case `Store::update_run` cannot express, and the reason this is not
                // simply a copy of `resume`'s fix.
                None => run,
                // Behind what we were handed: a grant carries the arbiter's decision, and this
                // node's row is a memory of the leg before it.
                Some(stored) if stored.epoch < run.epoch => run,
                // Otherwise the row wins. It is the only copy that has seen everything that
                // happened while the caller was holding its own.
                Some(stored) => stored,
            };
            // For the sentence, not for the fence: `assign` refuses a terminal run on its own,
            // and a terminal run holds no lease so it can never take the branch below. What
            // this adds is a reason somebody can read, in place of a transition error about a
            // state machine.
            if run.state.is_terminal() {
                return Err(SubmitError::Ended {
                    run: run_id.short(),
                    action,
                    state: run.state.name(),
                });
            }
            // **The lease, not `holder()`.** They differ for exactly one state and it is the one
            // that matters here: `Run::holder` answers `Some(last)` for an `Orphaned` run, so
            // asking it lets a run whose lease has been taken away take the already-ours branch
            // — held at an epoch nobody granted, which is the whole of what "`Orphaned` must
            // never return a live lease" forbids. Measured: the row stayed `orphaned`, the epoch
            // did not move, the agent was registered, and `drive`'s pre-spawn fence caught it a
            // cold clone later and called it a lost run. Reading the lease sends it to `assign`
            // instead, which is legal from `Orphaned` and is the reclaim — the same thing this
            // node does for a run *another* node last held, because the last holder's identity
            // is not authority.
            let epoch = match run.state.lease() {
                Some(lease) if lease.node == self.node_id => run.epoch,
                _ => match run.assign(self.node_id, now(), LEASE_TTL) {
                    Ok(epoch) => epoch,
                    // Somebody else is running it. `assign` refuses a non-terminal run for two
                    // reasons and this is the one that happens: the state is `Assigned`,
                    // `Running` or `Checkpointing` under a lease, and it is not ours, because a
                    // lease of ours took the branch above. `LostTheRun` is the variant that
                    // says so — what this returned before was `Agent("cannot assign a run that
                    // is running")`, a transition error in an operator's error message.
                    Err(_) if run.state.lease().is_some() => {
                        return Err(SubmitError::LostTheRun {
                            run: run_id.short(),
                            reason: format!(
                                "it is {} on {}",
                                run.state.name(),
                                run.holder().map_or_else(String::new, |n| n.short())
                            ),
                        });
                    }
                    // The other reason, and nothing in the product reaches it: a `Pinned` run
                    // with an attempt spent. Three gates stand in front of it — `release`
                    // refuses to return a pinned run to the pool, `reassign` holds it with
                    // `HoldReason::PinnedToHolder`, and a drain keeps it with
                    // `KeepReason::PinnedHere` — so a pinned run is never offered to anybody.
                    // A sentence rather than a variant, until something can produce it.
                    Err(e) => {
                        return Err(SubmitError::Refused {
                            run: run_id.short(),
                            action,
                            reason: e.to_string(),
                        });
                    }
                },
            };
            Ok((run, epoch))
        })?
    }

    /// Start a run on this node: assign it here if nobody has, and launch the agent.
    ///
    /// Takes a whole `Run` because it serves two callers that look different and are not: a
    /// local submission, and a run another node's arbiter granted to us. The second arrives
    /// already assigned at an epoch the fleet agreed on, and re-assigning it here would burn
    /// that epoch for nothing.
    ///
    /// **The copy it is handed decides nothing.** It says which run, and — when it is a grant —
    /// what the arbiter decided; the row decides whether this node may still start it. See
    /// [`Self::hold_here`].
    pub async fn start_run(&self, run: Run) -> Result<(), SubmitError> {
        let run_id = run.id;
        let (run, epoch) = self.hold_here(run, "started")?;

        // **Before anything this leg writes down.** `register` is what decides whether a second
        // agent starts (see it for what that cost), and a leg that is not going to start must not
        // leave an acceptance in the audit log or "preparing" on the run's summary on its way to
        // finding out. It is also what a concurrent `cancel_run` looks for, so claiming it early
        // is the safe direction on that side too.
        let mut cancel_rx = self.register(run_id, epoch)?;

        // **The one dispatch point.** Everything above is the same for both tiers — the hold,
        // the epoch, the exclusive claim — because it is about a *run* rather than about what
        // the run does. Everything below differs, and a task differs by having less: no
        // repository to resolve, no workspace to prepare, no transcript to fetch, so
        // `prepare_start` has nothing to ask (ADR-0019 §2).
        let work = match &run.spec.work {
            Work::Agent(work) => work.clone(),
            Work::Task(task) => {
                let task = task.clone();
                self.note_accepted(run_id, epoch);
                self.launch_task(run_id, task, cancel_rx);
                return Ok(());
            }
        };
        let source = RepoSource::parse(&work.workspace.repo);
        let git_ref = work.workspace.git_ref.clone();
        // The choke point every started run passes through — a submission on a fleet of one, a
        // grant from an arbiter, a resume — which is why the acceptance is recorded here and not
        // at any of the three. Recording it at `take_run` instead missed the commonest case in
        // this project by far: a node with no mesh never goes near it.
        self.note_accepted(run_id, epoch);
        // Deliberately not a reset. A run arriving here with a checkpoint has done turns and
        // spent money somewhere else, and `RunStats::default()` — which is what this used to
        // write — is how a migrated run came out the far side reporting that it had done
        // neither. What starting means for the numbers is a new *leg*, not a new run.
        self.note_workspace(run_id, "preparing");

        // Everything between the registration and `launch` can fail, and `launch` is what
        // installs the arm that cleans up after a failure — so this one has to be given back by
        // hand. Leaving it is not untidy, it is a wedge: `held_but_not_started` filters on
        // `live`, so the run never comes round again, and `started_excluding` counts
        // live-or-`Running` for a run this node holds, so the slot stays occupied by an agent
        // that was never started. Measured on the ordinary way in — a migration whose
        // transcript cannot be fetched.
        let start = match self.prepare_start(run_id, &run).await {
            Ok(start) => start,
            Err(e) => {
                self.unregister(run_id, &mut cancel_rx).await;
                return Err(e);
            }
        };

        self.launch(run_id, source, git_ref, cancel_rx, start);
        Ok(())
    }

    /// What the agent is to be started with: a fresh prompt, or the conversation so far.
    ///
    /// A run granted with a checkpoint is a migration: the conversation and the uncommitted
    /// work exist somewhere and have to be here before the agent restarts. Anything else would
    /// resume an empty worktree and call it the same run.
    async fn prepare_start(&self, run_id: RunId, run: &Run) -> Result<Start, SubmitError> {
        let work = run.spec.agent().ok_or(SubmitError::UnsupportedWork {
            kind: run.spec.work.kind().name(),
        })?;
        let submitted = LogKind::Submitted {
            repo: work.workspace.repo.clone(),
            prompt: work.prompt.clone(),
            permission: format!("{:?}", work.permission_mode),
        };
        if let (Some(base), Some(parent)) = (run.continuation_base(), run.spec.parent.clone()) {
            let checkpoint = self.ensure_blobs(run_id, Box::new(base.clone())).await?;
            self.append(run_id, submitted);
            return Ok(Start::Continue { checkpoint, parent });
        }
        match run.checkpoint.clone() {
            Some(checkpoint) => {
                let session =
                    checkpoint
                        .session_id
                        .clone()
                        .ok_or_else(|| SubmitError::Refused {
                            run: run_id.short(),
                            action: "resumed",
                            reason: "its checkpoint records no agent session".to_string(),
                        })?;
                let checkpoint = self.ensure_blobs(run_id, Box::new(checkpoint)).await?;
                self.append(run_id, submitted);
                Ok(Start::Resume {
                    prompt: offload_agent::claude::CONTINUE_PROMPT.to_string(),
                    session: SessionId(session),
                    checkpoint,
                })
            }
            None => {
                self.append(run_id, submitted);
                Ok(Start::Fresh)
            }
        }
    }

    /// Undo [`Self::register`] for a start that gave up before the agent existed — **honouring a
    /// cancel that arrived in the window rather than dropping it.**
    ///
    /// The registration is what a concurrent `cancel_run` finds, and finding it is the whole of
    /// what it does: `halt_agent` takes the sender, signals, and answers "its agent was stopped"
    /// *without writing anything*, because the writer is `drive`. Give the registration back
    /// with a halt still in the channel and there is no writer left — the run stays `Assigned`,
    /// the operator has been told it stopped, and the next tick starts it.
    ///
    /// The entry and the sender go under one lock, so the race has only two outcomes: either the
    /// sender is still here and no cancel can ever be accepted, or somebody has it and their
    /// message is on its way, which is why the wait below is bounded.
    async fn unregister(&self, run_id: RunId, cancel_rx: &mut mpsc::Receiver<Halt>) {
        let ours = lock(&self.live)
            .remove(&run_id)
            .and_then(|entry| entry.cancel);
        if ours.is_some() {
            return;
        }
        // `Superseded` is deliberately not written down: the run is somebody else's now and the
        // record is theirs (see `Halt`), and this leg giving up is exactly what it asked for.
        if let Some(Halt::Cancelled { by }) = cancel_rx.recv().await {
            self.finish_cancelled(run_id, &by);
            self.note_workspace(run_id, NEVER_STARTED);
        }
    }

    /// Make sure every blob a checkpoint names is on this machine, and say so in the type.
    ///
    /// The common migration fetches nothing: the checkpoint was replicated here precisely
    /// because this node was the plausible successor (ADR-0016). When it does have to fetch,
    /// failing here is the right place — before the agent starts, rather than in the middle
    /// of a restore that leaves a half-built worktree.
    ///
    /// **Every path to `Start::Resume` comes through here**, which is the point of returning a
    /// [`Restorable`] rather than `()`: this was a rule with two doors and one guard, and
    /// `resume` was the unguarded one for as long as it has existed.
    ///
    /// The store is asked **twice**, and the second answer is the one that decides. A `fetch`
    /// that reports success without leaving the bytes here is otherwise indistinguishable from
    /// one that worked, and the place that finds out is `restore_workspace`, three awaits later
    /// and with a worktree half built.
    async fn ensure_blobs(
        &self,
        run_id: RunId,
        checkpoint: Box<Checkpoint>,
    ) -> Result<Restorable, SubmitError> {
        let missing = restorable::missing(&checkpoint, |hash| self.store.has_blob(hash));
        if !missing.is_empty() {
            let Some(peers) = self.peers.get().cloned() else {
                return Err(SubmitError::Refused {
                    run: run_id.short(),
                    action: "resumed",
                    reason: format!(
                        "{} of its checkpoint's blobs are not on this node, and it has no \
                         fleet to ask",
                        missing.len()
                    ),
                });
            };
            for hash in missing {
                tracing::info!(run_id = %run_id, blob = %hash.short(), "fetching checkpoint blob");
                peers.fetch(hash).await.map_err(|e| {
                    SubmitError::Agent(format!("fetching checkpoint blob {}: {e}", hash.short()))
                })?;
            }
        }

        Restorable::check(checkpoint, |hash| self.store.has_blob(hash)).map_err(|absent| {
            SubmitError::Agent(format!(
                "{} checkpoint blob(s) are still not here after fetching, starting with {}",
                absent.len(),
                absent
                    .first()
                    .map_or_else(String::new, offload_core::BlobHash::short)
            ))
        })
    }

    /// Acquire an archive workspace's bytes and unpack them into this node's mirror.
    ///
    /// ADR-0061 §1 and §2. `ensure_blobs`' shape applied to the *workspace* rather than the
    /// checkpoint, and for the same reason: `WorkspaceManager` has no network and must not grow
    /// one, so the thing that can fetch does the fetching and hands over a path.
    ///
    /// Called before every start, fresh or resumed, and cheap once the mirror exists — an
    /// archive is content-addressed, so a mirror that is there is the right one for ever and
    /// this is one `is_warm` check. The acquisition happens **once per node per run** (§3),
    /// which is what keeps the bulk out of the per-turn checkpoint.
    ///
    /// A path rather than the bytes: a 512 MiB blob costs its whole size in RAM at each end of
    /// the transfer already (measured, session seventy-eight), and there is no reason for the
    /// workspace layer to pay it a third time.
    async fn ensure_archive(&self, run_id: RunId, source: &RepoSource) -> Result<(), SubmitError> {
        let Some(hash) = source.archive() else {
            return Ok(());
        };
        if self.workspaces.is_warm(source) {
            return Ok(());
        }
        if !self.store.has_blob(hash) {
            let Some(peers) = self.peers.get().cloned() else {
                return Err(SubmitError::Refused {
                    run: run_id.short(),
                    action: "started",
                    reason: format!(
                        "its workspace is the archive {}, which is not on this node and there \
                         is no fleet to ask",
                        hash.short()
                    ),
                });
            };
            tracing::info!(run_id = %run_id, archive = %hash.short(), "fetching an archive workspace");
            peers.fetch(hash).await.map_err(|e| {
                SubmitError::Agent(format!("fetching archive workspace {}: {e}", hash.short()))
            })?;
        }
        // Asked again rather than assumed, for the reason `restore_workspace` asks the store
        // twice: a `fetch` that returns `Ok` without leaving the bytes here is otherwise
        // indistinguishable from one that worked, and the place that would find out is halfway
        // through building a workspace.
        if !self.store.has_blob(hash) {
            return Err(SubmitError::Agent(format!(
                "the archive {} is still not here after fetching it",
                hash.short()
            )));
        }
        let path = offload_store::blobs::blob_path(&self.store.blob_root(), hash);
        let (_, acquired) = self
            .workspaces
            .ensure_archive_mirror(source, &path)
            .await
            .map_err(|e| SubmitError::Agent(e.to_string()))?;
        // Only when bytes actually moved (§3). A node that already held this archive has nothing
        // to report, and a line claiming an acquisition that did not happen would be worse than
        // no line — it would put a second "what travelled" answer in the log of a run whose
        // workspace has not changed since the first.
        if let Some(acquired) = acquired {
            self.append(
                run_id,
                LogKind::WorkspaceAcquired {
                    archive: hash.short(),
                    summary: acquired.to_string(),
                },
            );
        }
        Ok(())
    }

    /// Put a continuation's parent transcript at [`offload_core::PARENT_TRANSCRIPT`] in its
    /// worktree, fetching the blob if this node does not have it, and keep `.offload/` out of
    /// git (ADR-0064 §3). Nothing to do for a continuation that was handed no transcript.
    async fn materialise_parent_transcript(
        &self,
        run_id: RunId,
        source: &RepoSource,
        workspace: &Workspace,
        parent: &offload_core::Continuation,
    ) -> Result<(), SubmitError> {
        let Some(hash) = parent.transcript else {
            return Ok(());
        };
        self.workspaces
            .exclude_offload_dir(source)
            .map_err(|e| SubmitError::Agent(e.to_string()))?;
        if !self.store.has_blob(hash) {
            let Some(peers) = self.peers.get().cloned() else {
                return Err(SubmitError::Refused {
                    run: run_id.short(),
                    action: "started",
                    reason: "its parent's transcript is not on this node and there is no fleet \
                             to ask"
                        .to_string(),
                });
            };
            peers.fetch(hash).await.map_err(|e| {
                SubmitError::Agent(format!(
                    "fetching the parent's transcript {}: {e}",
                    hash.short()
                ))
            })?;
        }
        let bytes = self.store.get_blob(hash)?;
        let path = workspace.path.join(offload_core::PARENT_TRANSCRIPT);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| SubmitError::Agent(format!("creating {}: {e}", dir.display())))?;
        }
        std::fs::write(&path, bytes)
            .map_err(|e| SubmitError::Agent(format!("writing {}: {e}", path.display())))?;
        Ok(())
    }

    /// Remember a run this node is not running: one it placed elsewhere, or heard about.
    ///
    /// ADR-0006 requires it — a pending run held only in view memory is lost at the next
    /// restart, and the submitting node is exactly the one about to be shut.
    ///
    /// It is also where **fencing becomes an action**. Learning that a run we are running has
    /// moved to a higher epoch somewhere else is the moment to stop our agent: two agents on
    /// one repository, both committing, is the failure this whole design exists to prevent,
    /// and the epoch is the only thing that can tell them apart.
    /// Change the editable part of a run's spec, as the node that owns it (ADR-0013).
    ///
    /// Returns the amended run and the sentence to say back, because what the change *does*
    /// depends entirely on the run's state and only this side knows it:
    ///
    /// * **Pending** — it genuinely changes something: the run is offered again and judged
    ///   against the new deadline, or offered ahead of the runs it now outranks.
    /// * **Running** — it changes nothing anybody can see while things go well. There is no
    ///   preemption, no way to make an agent think faster, and no run is stopped to let another
    ///   one past. What it buys is different behaviour when the run *breaks*: how long its
    ///   holder is waited for, how patiently a failure is retried, which run goes first the next
    ///   time something has to choose. Saying so is the difference between a command that is
    ///   understood and one that will be reported as broken by whoever expected it to hurry.
    /// * **Finished** — refused. There is no decision left for it to influence, and writing a
    ///   revision that changes nothing would look like it had worked.
    pub fn edit_spec(
        &self,
        run_id: RunId,
        edit: offload_core::SpecEdit,
    ) -> Result<(Run, String), SubmitError> {
        // Read, refused-or-edited, and written under one lock. The revision is the whole of
        // what settles this field against the fleet's copies, so two edits that each read the
        // same revision and wrote the next one would leave one of them believed everywhere and
        // the other believed nowhere, with both having said "done" at the keyboard.
        let edited = self
            .store
            .update_run(run_id, |run| {
                if run.state.is_terminal() {
                    return Err(SubmitError::Refused {
                        run: run_id.short(),
                        action: match edit {
                            offload_core::SpecEdit::Deadline { at: _ } => "given a deadline",
                            offload_core::SpecEdit::Priority { to: _ } => "re-prioritised",
                        },
                        reason: format!("it has already {}", run.state.name()),
                    });
                }
                let rev = run.edit(edit);
                Ok((run.clone(), rev))
            })?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;
        let (run, rev) = edited?;
        tracing::info!(run_id = %run_id, rev, %edit, "spec edited");

        // **Three states, not two.** This asked `pending?` and answered everything else with
        // "it is already under way", which is false for the state where both edits do the most:
        // a run this node has accepted and not started, which `offload ps` calls *waiting for a
        // slot*. Measured — a deadline put on the older of two queued runs sent it from first to
        // last, while the note said the change affected "only how long the fleet waits for it if
        // something goes wrong", and a priority of 10 on a run that then started second was
        // reported as deciding "the next time something has to choose".
        //
        // The position comes from `held_but_not_started`, which is the function that *decides*
        // the order, read back after the edit. Anything else here would be a second copy of
        // `urgency_order` disagreeing with the one that runs.
        let waiting_here = self.held_but_not_started();
        let queued = waiting_here
            .iter()
            .position(|r| r.id == run_id)
            .map(|at| (at, waiting_here.len()));
        let pending = matches!(run.state, RunState::Pending { .. });
        let note = match (edit, pending, queued) {
            (offload_core::SpecEdit::Deadline { at: _ }, true, _) => {
                "it is pending, so it is offered again and judged against the new deadline".into()
            }
            (offload_core::SpecEdit::Priority { to: _ }, true, _) => {
                "it is pending, so it is offered ahead of the runs it now outranks — and behind \
                 any that are more overdue, because urgency leads and priority breaks its ties"
                    .into()
            }
            // Accepted here and not started: the one place in this system where an order exists
            // at all, and the one the operator is looking at when they type either command.
            (edit, false, Some((at, of))) => queue_note(edit, at, of),
            (offload_core::SpecEdit::Deadline { at: Some(_) }, false, None) => {
                "it is already under way — nothing speeds an agent up, so this changes only how \
                 long the fleet waits for it if something goes wrong"
                    .into()
            }
            (offload_core::SpecEdit::Deadline { at: None }, false, None) => {
                "it is already under way; it is due as soon as it can be, as it was before \
                 anybody said otherwise"
                    .into()
            }
            (offload_core::SpecEdit::Priority { to: _ }, false, None) => {
                "it is already under way, and nothing is preempted for it: this decides only \
                 which run goes first the next time something has to choose"
                    .into()
            }
        };
        Ok((run, note))
    }

    /// Take an operator's edit to a run this node is holding, and nothing else from the record
    /// it arrived on.
    ///
    /// [`Self::record_run`]'s counterpart, and the reason the two are not one function: that one
    /// writes a record whole, which is right for a run living somewhere else and wrong for one
    /// running here. The store's copy is what a resume reads, what orders the runs waiting for a
    /// slot, and what decides whether a late commitment is handed back — and between the gossip
    /// tick that published our record and the edit coming back, this run may have checkpointed
    /// or finished. So the edit is applied to the row as it stands rather than carried in on a
    /// copy of it, under the store's own lock (`Store::update_run`).
    pub fn apply_spec_edit(&self, incoming: &Run) -> Result<(), SubmitError> {
        // Both directions of the rule live in `settle_spec`; the copy is what it settles
        // against and is then discarded.
        let mut incoming = incoming.clone();
        let moved = self
            .store
            .update_run(incoming.id, |stored| {
                offload_core::settle_spec(stored, &mut incoming)
            })
            .map_err(|e| SubmitError::Agent(e.to_string()))?;
        if moved == Some(true) {
            tracing::info!(
                run_id = %incoming.id,
                rev = incoming.spec_rev,
                "took an edit to a run we are holding"
            );
        }
        Ok(())
    }

    pub fn record_run(&self, run: &Run) -> Result<(), SubmitError> {
        // **Finished is finished, in the store as well as the view.** A record from the fleet is
        // written whole because the view's merge has settled it, but the view forgets finished
        // runs after `GOSSIP_TAIL`, and past that there was no merge at all: a device that had
        // been away sent its `pending` copy of a run cancelled hours before, the view took it as
        // a run it had never heard of, and this wrote it over the stored `cancelled` (the laptop,
        // session ninety-four). The view's own rule, asked of the durable copy: a live record at
        // the same or a lower epoch says nothing about a finished one. A higher epoch still wins,
        // which is the ordinary handover and `finished_and_lost` below. And the sender is behind,
        // so the finished record is told again for it.
        if !run.state.is_terminal() {
            if let Some(stored) = self.store.load_run(run.id)? {
                if stored.state.is_terminal() && run.epoch <= stored.epoch {
                    tracing::debug!(
                        run_id = %run.id,
                        stored = %stored.state.name(),
                        "a peer sent a stale live copy of a finished run; keeping ours, telling it again"
                    );
                    self.retell(run.id);
                    return Ok(());
                }
            }
        }

        // `<=`, not `<`. A higher epoch is the ordinary reassignment and was always caught; an
        // *equal* one naming somebody else is a run granted twice at one token, which the merge
        // has now settled against us (`ClusterView::merge_run`). Reading it as "not our
        // business" is what let two agents keep going on one repository, each perfectly happy,
        // for as long as the run lasted — the very thing the epoch exists to make impossible.
        // Our own leg is excluded by the holder check above it, so there is no legitimate case
        // where the record names another node at the epoch we are running under.
        let held = lock(&self.live)
            .get(&run.id)
            .filter(|live| live.epoch <= run.epoch && live.cancel.is_some())
            .map(|live| live.epoch);
        let superseded = run.holder() != Some(self.node_id) && held.is_some();

        // …and the same loss after the leg has **ended**: no agent is live, so `held` above is
        // `None`, and the record this node wrote — completed, at the lower epoch — is about to be
        // overwritten by the fleet's. Reached by a daemon frozen across its agent's last turn
        // (measured, session ninety): on thawing it read the agent's result before the gossip,
        // completed the run at epoch 1, and bravo's epoch-2 record replaced it with nothing on
        // alpha saying its whole leg was not the run's. `Completed` only: a *failed* leg picked
        // up elsewhere is the ordinary handover (ADR-0043), which carries its work forward.
        let finished_and_lost = if held.is_none() && run.holder() != Some(self.node_id) {
            self.store
                .load_run(run.id)
                .ok()
                .flatten()
                .filter(|stored| {
                    matches!(stored.state, RunState::Completed { .. }) && stored.epoch < run.epoch
                })
                .filter(|_| self.progress_leg(run.id) == Some(self.node_id))
                .map(|stored| stored.epoch)
        } else {
            None
        };

        self.store.save_run(run)?;

        if let (true, Some(held), Some(to)) = (superseded, held, run.holder()) {
            // In the audit log as well as in `tracing`, because this is the row that answers the
            // question the position rule creates: a run's turn count follows the leg its record
            // settled on, so it drops back to the survivor's here — and this node is the only one
            // that can say the higher number belonged to work that is no longer the run's. The
            // surviving leg never learns there was another one.
            self.note_superseded(run.id, held, to, run.epoch, false);
        }
        // Named only when the record names the holder: one that arrives already finished names
        // nobody, and a guess at who ran it would be this row's one false word. That arm is a
        // residual, in `docs/HANDOFF.md`.
        if let (Some(held), Some(to)) = (finished_and_lost, run.holder()) {
            tracing::warn!(
                run_id = %run.id,
                held = held.0,
                epoch = run.epoch.0,
                "this node finished a run the fleet had already given to another node"
            );
            self.note_superseded(run.id, held, to, run.epoch, true);
        }

        if superseded {
            tracing::warn!(
                run_id = %run.id,
                epoch = run.epoch.0,
                holder = %run.holder().map(|n| n.short()).unwrap_or_default(),
                "this run has been reassigned; stopping the agent here"
            );
            let this = self.clone();
            let id = run.id;
            tokio::spawn(async move {
                this.halt_agent(id, Halt::Superseded).await;
            });
        }
        Ok(())
    }

    /// Take a run that stopped and start its agent again from its last checkpoint.
    ///
    /// The three ways a run can be waiting for this are all handled here: released by
    /// `offload checkpoint` (`Pending`), interrupted by a daemon restart (`Failed`), or
    /// simply still around from an earlier resume. What is refused is a run that finished
    /// on purpose — `Completed` and `Cancelled` are decisions, and re-opening one would
    /// restart work somebody deliberately stopped.
    pub async fn resume(
        &self,
        run_id: RunId,
        prompt: Option<String>,
        room: impl Into<offload_core::Room>,
    ) -> Result<(), SubmitError> {
        let run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;

        // The state is read here for the refusals, and **decided again inside the store's lock
        // below** — this copy can be stale by the time anything acts on it. Everything from here
        // to the transition is a read-only question about a run that has stopped.
        refuse_unresumable(&run, run_id, "resumed")?;

        if lock(&self.live)
            .get(&run_id)
            .is_some_and(|l| l.cancel.is_some())
        {
            return Err(SubmitError::Refused {
                run: run_id.short(),
                action: "resumed",
                reason: "its agent is still shutting down here; try again in a moment".to_string(),
            });
        }

        let Some(checkpoint) = run.checkpoint.clone() else {
            return Err(SubmitError::Refused {
                run: run_id.short(),
                action: "resumed",
                reason: "it has no checkpoint — there is no conversation to continue".to_string(),
            });
        };
        // A continuation that failed before its first capture still holds only its *base* — the
        // parent's work at turn zero — which has no session of its own in handoff mode. Picking it
        // up again means starting it again from that base, exactly as its first start did
        // (ADR-0064); refusing it for want of a session left the recovery tick, and a person, with
        // no way back to a run that had lost nothing.
        enum Leg {
            FromBase(offload_core::Continuation),
            Session(String),
        }
        let leg = match (
            run.continuation_base().and(run.spec.parent.clone()),
            checkpoint.session_id.clone(),
        ) {
            (Some(parent), _) => Leg::FromBase(parent),
            (None, Some(session)) => Leg::Session(session),
            (None, None) => {
                return Err(SubmitError::Refused {
                    run: run_id.short(),
                    action: "resumed",
                    reason: "its checkpoint records no agent session".to_string(),
                })
            }
        };

        // The limit again, at the other end of it. The boundary check stops a run that reaches
        // its budget; this stops one that has *already* reached it from being started again —
        // and without it the cap is a suggestion, since `resume` is offered by name in the
        // sentence a failed run prints. The count is the checkpoint's rather than the live
        // stats': it is the turn a resumed agent would be numbering from, so it is the number
        // the question is actually about.
        //
        // Refused rather than allowed-because-a-person-asked, which is the tempting reading —
        // an operator at the keyboard is present, and presence is what `Attendance` lets decide
        // elsewhere. It does not apply here: attendance decides whether to act *without* being
        // asked, and this is somebody asking for one more turn than they said they wanted. The
        // way to get more turns is to say a bigger number, and `max_turns` is fixed at
        // submission, so today that means a new run. Stated, rather than worked around by
        // letting the limit be spent twice.
        if let Some(limit) = run.spec.turn_limit_reached(checkpoint.turns) {
            return Err(SubmitError::Refused {
                run: run_id.short(),
                action: "resumed",
                reason: format!(
                    "it reached its turn limit of {limit} — start a new run to go further"
                ),
            });
        }

        // …and the fourth door. `resume` is the one a person types **and** the one the recovery
        // tick uses unasked, and it restarts an agent exactly as a submission does — which is
        // the argument ADR-0047 already made about the *hosting* gate, met again one ceiling
        // over.
        if let Some(refusal) = self.agent_full(&run, |kind| self.held_for_agent(kind)) {
            return Err(SubmitError::NoRoom(refusal));
        }
        self.room(room)
            .for_one_more(
                self.with_device(self.held()),
                run.spec
                    .agent_kind()
                    .map_or(0, |kind| self.held_for_agent(kind)),
                run.spec.demand,
            )
            .map_err(SubmitError::NoRoom)?;

        // **Before the claim, not after it.** A resume on a node the checkpoint was never
        // replicated to has to fetch the conversation from whoever has it, and until this
        // existed nothing on this path did: `start_run` fetched, `resume` did not, and the two
        // are the only ways an agent is ever restarted from a checkpoint. What that cost is in
        // `docs/pitfalls/checkpoints-blobs-and-workspaces.md` — the run died inside
        // `restore_workspace` with `blob … is not stored here`, which reads as corruption
        // rather than as a fetch nobody made.
        //
        // Ahead of the claim because everything it can go wrong with is a refusal this run
        // would meet whoever asked: no epoch is spent, no lease is taken, nothing has to be
        // given back, and a `Failed` run is not reopened into `Assigned` only to sit there. It
        // is an `await` on a stale copy, like every read above it, and the state is decided
        // again under the store's lock below.
        let checkpoint = self.ensure_blobs(run_id, Box::new(checkpoint)).await?;

        // **Read, decide and write under one lock**, because this function has two callers with
        // different intentions — a person typing `offload resume`, and the recovery tick picking
        // an unattended failure back up — and nothing schedules them apart. The version this
        // replaces loaded the run, spent two full `list_runs` scans on the capacity check above,
        // and only then assigned and saved: a second caller loading inside that window would
        // assign from the same copy, compute the same epoch, and launch a second agent into the
        // same worktree that every later fence would wave through, since both legs hold the run
        // at the epoch they agree on. `Store::update_run`'s own doc comment describes this
        // hazard and exists to prevent it — "the window does not need an `await` in it to be
        // real: two tokio tasks on one store are enough" — and this was the load-modify-save it
        // warns about, in the one place whose failure is two agents committing to one repo.
        //
        // The state is checked again in here rather than only above: the check that matters is
        // the one taken with the write, and the loser of a race now reads the winner's
        // `Assigned` and is refused with the sentence it would have got anyway.
        let epoch = self
            .store
            .update_run(run_id, |run| {
                refuse_unresumable(run, run_id, "resumed")?;
                if matches!(run.state, RunState::Failed { .. }) {
                    run.reopen(now())
                        .map_err(|e| SubmitError::Agent(e.to_string()))?;
                }
                run.assign(self.node_id, now(), LEASE_TTL)
                    .map_err(|e| SubmitError::Agent(e.to_string()))
            })?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))??;

        // **`resume` is the agent tier's door, and a task has nothing for it to open.**
        // Resuming means resuming a conversation (ADR-0019 §2), which is also why
        // `RunSpec::check` refuses `Resumable` for a task at submission — so a task reaching
        // here has already been refused once and this is the belt to that braces. What a
        // person wants for a failed task is to run it again, which is `offload run`, and
        // making this door quietly mean that would be two verbs sharing a word.
        let work = run.spec.agent().ok_or(SubmitError::UnsupportedWork {
            kind: run.spec.work.kind().name(),
        })?;
        let source = RepoSource::parse(&work.workspace.repo);
        let git_ref = work.workspace.git_ref.clone();
        // The check above is the one that says the useful sentence; this is the one that is
        // taken *with* the claim, and only it can be trusted — the live map can change between
        // the two, and nothing schedules `resume`'s two callers apart.
        let cancel_rx = self.register(run_id, epoch)?;

        self.launch(
            run_id,
            source,
            git_ref,
            cancel_rx,
            match leg {
                // From the base again, with the spec's own prompt: the conversation it would
                // continue never existed.
                Leg::FromBase(parent) => Start::Continue { checkpoint, parent },
                Leg::Session(session) => Start::Resume {
                    prompt: prompt
                        .unwrap_or_else(|| offload_agent::claude::CONTINUE_PROMPT.to_string()),
                    session: SessionId(session),
                    checkpoint,
                },
            },
        );
        Ok(())
    }

    /// Run a failed task's program again, from the spec it was submitted with (ADR-0058).
    ///
    /// **The tick's door, and only the tick's.** `decide_recovery` answers `Recovery::Resume` for
    /// an unattended failed task — deliberately, because `Restartability::Idempotent` is on every
    /// task and ADR-0043 already has a departing node hand one to the fleet to be re-run — and
    /// the mechanism behind that answer was [`Self::resume`], which refuses a task on its fourth
    /// line for want of a conversation. So the tick asked every 30s, was refused instantly,
    /// **counted the refusal as a spent retry**, and gave up after three; `offload explain`
    /// meanwhile promised *"picking it up again in 16.7s (try 1)"*, and the warning in the log
    /// said a shell script had *"no conversation to continue"*. Measured on two daemons walking
    /// phase 8's demo.
    ///
    /// Its own method rather than an arm inside `resume`, because that door is the one a **person**
    /// types at and its refusal there is a decision, not a gap: resuming means continuing a
    /// conversation, and what somebody wants for a failed task is to run it again, which is
    /// `offload run --task`. Two verbs, two doors, one caller each — and the counting is the
    /// lesson this file has paid for, so: the ways to start a task are now `start_run`'s task arm
    /// and this, and both go through `launch_task`, whose pre-spawn fence is what actually stops
    /// two of the owner's programs at once.
    ///
    /// Everything `resume` does that a task has no use for is absent rather than skipped: no
    /// checkpoint, no session id, no blob fetch, no turn limit — a task has no turn boundary, so
    /// `max_turns` cannot apply to one and `RunSpec::check` refuses the pairing at submission.
    /// What is kept is the claim: the state is decided again inside the store's lock, because
    /// this function and `resume` have no scheduling between them.
    pub async fn restart_task(
        &self,
        run_id: RunId,
        room: impl Into<offload_core::Room>,
    ) -> Result<(), SubmitError> {
        let run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;

        // Read for the sentence, and decided again under the lock below — `resume`'s rule, and
        // for its reason: this copy can be stale by the time anything acts on it.
        refuse_unresumable(&run, run_id, "restarted")?;
        // Belt to `decide_recovery`'s braces. Nothing else should ever reach here with an agent
        // run, and if something does, the honest answer is the one `resume`'s tail gives for a
        // task: this door is for the other tier.
        if run.spec.work.kind() != offload_core::WorkKind::Task {
            return Err(SubmitError::UnsupportedWork {
                kind: run.spec.work.kind().name(),
            });
        }
        if lock(&self.live)
            .get(&run_id)
            .is_some_and(|l| l.cancel.is_some())
        {
            return Err(SubmitError::Refused {
                run: run_id.short(),
                action: "restarted",
                reason: "its program is still being stopped here; try again in a moment"
                    .to_string(),
            });
        }

        // The owner's budget, asked before the claim for `resume`'s reason: a refusal for want of
        // room costs no epoch and leaves nothing to give back. `held_for_agent` is nought by
        // construction — a task names no agent — which is why the pair is passed rather than
        // assumed.
        self.room(room)
            .for_one_more(
                self.with_device(self.held()),
                run.spec
                    .agent_kind()
                    .map_or(0, |kind| self.held_for_agent(kind)),
                run.spec.demand,
            )
            .map_err(SubmitError::NoRoom)?;

        let epoch = self
            .store
            .update_run(run_id, |run| {
                refuse_unresumable(run, run_id, "restarted")?;
                if matches!(run.state, RunState::Failed { .. }) {
                    run.reopen(now())
                        .map_err(|e| SubmitError::Agent(e.to_string()))?;
                }
                run.assign(self.node_id, now(), LEASE_TTL)
                    .map_err(|e| SubmitError::Agent(e.to_string()))
            })?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))??;

        let Some(task) = run.spec.task().cloned() else {
            return Err(SubmitError::UnsupportedWork {
                kind: run.spec.work.kind().name(),
            });
        };
        // The claim before the launch, and the launch's own fence after it: `register` is what
        // decides whether a second program starts, and `drive_task` re-reads the row immediately
        // before the spawn. Same order `start_run`'s task arm uses, for the same reason.
        let cancel_rx = self.register(run_id, epoch)?;
        self.note_accepted(run_id, epoch);
        self.launch_task(run_id, task, cancel_rx);
        Ok(())
    }

    /// Ask the holder to checkpoint at its next turn boundary and give the run up.
    ///
    /// Deliberately not "checkpoint now": an agent halfway through a tool call has state
    /// in the tool rather than the transcript, so capturing there would produce a
    /// checkpoint that restores into a worktree the conversation does not describe.
    pub fn request_checkpoint(&self, run_id: RunId, by: GivenUp) -> Result<(), SubmitError> {
        {
            let mut live = lock(&self.live);
            let entry = live.get_mut(&run_id).filter(|l| l.cancel.is_some()).ok_or(
                SubmitError::Refused {
                    run: run_id.short(),
                    action: "checkpointed",
                    reason: "it is not running here, so there is no turn boundary coming"
                        .to_string(),
                },
            )?;
            self.store
                .update_run(run_id, |run| run.request_checkpoint(now()))?
                .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?
                .map_err(|e| SubmitError::Agent(e.to_string()))?;
            // **A departure is not downgraded by a later request.** Both callers can reach one
            // run: a drain waits minutes, and a person can type `offload checkpoint` inside that
            // window. The node is leaving either way, so `LetGo` is the fact that survives — and
            // getting this backwards writes `Parked` on a run nobody will come back for, which
            // is the exact stranding ADR-0042 exists to stop.
            entry.checkpoint_requested = Some(match (entry.checkpoint_requested, by) {
                (Some(GivenUp::LetGo), _) | (_, GivenUp::LetGo) => GivenUp::LetGo,
                _ => GivenUp::Parked,
            });
        }

        tracing::info!(run_id = %run_id, ?by, "checkpoint requested; will capture at the next turn boundary");
        Ok(())
    }

    /// Make a run followable and cancellable, and hand back its cancel channel — **or refuse,
    /// because an agent for it is already going here.**
    ///
    /// The channel carries a [`Halt`] rather than a unit, because "stop the agent" and "the run
    /// was cancelled" are two facts and only one of them is this node's to record.
    ///
    /// **This is the one place that decides whether a second agent starts**, and it is here
    /// rather than at the call sites for the reason `Supervisor::holds` is in the writer and
    /// ADR-0050's rendezvous is in `Cluster::place`: four callers checking is four things to
    /// remember, and the fifth will not. The check and the insert are one lock, so two legs
    /// cannot both read "nobody is going" and then both claim it — the read-decide-write shape
    /// that put `hold_here` under `update_run` in the first place, one map further out.
    ///
    /// What it costs when it is missing: this used to `insert` unconditionally, which **replaces**
    /// the running leg's entry. The displaced leg keeps its `cancel_rx` — the sender is dropped
    /// with the old entry, so nothing can ever stop it again — and goes on to `drive`, so two
    /// tasks build a workspace for one run. Both call `git worktree add -B <branch> <path>`, and
    /// git's own check is a read-decide-write too: of 200 forced races at one path, 17 came out as
    /// `fatal: cannot force update the branch 'X' used by worktree at 'X'` — the loser passing
    /// `die_if_checked_out` before the winner registers and failing inside `create_branch` after. That is the sentence in the session fifty-two logs, and it can be
    /// produced by nothing else here: `prepare` passes no `--force`, and without a concurrent
    /// add the same collision reads `'X' is already used by worktree at 'X'` instead. The run
    /// then ends `failed` with a raw git error, and the healthy leg is fenced out by the terminal
    /// state the colliding one wrote.
    ///
    /// The entry is still *replaced* where no agent is going: a [`LiveRun`] outlives its leg (see
    /// [`Self::agent_here`]), and a new leg of a run that came back is entitled to a fresh stream.
    fn register(&self, run_id: RunId, epoch: Epoch) -> Result<mpsc::Receiver<Halt>, SubmitError> {
        let mut live = lock(&self.live);
        if Self::agent_here(&live, run_id) {
            let going = live.get(&run_id).map_or(epoch, |leg| leg.epoch);
            // Loud, and here rather than at the caller, because **which** two callers met is the
            // open half of this: the entry in `docs/pitfalls/fencing-and-epochs.md` has the
            // symptom measured twice in eighty daemon passes and no door named. Whichever one it
            // is, it says so now — and the epochs say whether it was a re-grant (they differ) or
            // two legs born equal (they do not).
            tracing::warn!(
                run_id = %run_id,
                going = going.0,
                asked = epoch.0,
                "an agent for this run is already going here; not starting a second one"
            );
            return Err(SubmitError::AlreadyGoing {
                run: run_id.short(),
            });
        }
        let (stream, _) = broadcast::channel(256);
        let (cancel_tx, cancel_rx) = mpsc::channel(1);
        live.insert(
            run_id,
            LiveRun {
                epoch,
                live: Some(stream),
                cancel: Some(cancel_tx),
                checkpoint_requested: None,
            },
        );
        Ok(cancel_rx)
    }

    fn launch(
        &self,
        run_id: RunId,
        source: RepoSource,
        git_ref: Option<String>,
        cancel_rx: mpsc::Receiver<Halt>,
        start: Start,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            let outcome = this.drive(run_id, source, git_ref, cancel_rx, start).await;
            match outcome {
                Ok(()) => {}
                // The run is somebody else's now. `fail` would refuse this on its own — the guard
                // lives on the writer — so what this arm adds is the two things the guard cannot:
                // an honest log level, because "run failed" is the wrong sentence for a leg that
                // lost a race, and letting go of the process handle, which `drive` returned
                // before reaching.
                Err(e @ SubmitError::LostTheRun { .. }) => {
                    tracing::warn!(run_id = %run_id, error = %e, "this leg lost the run");
                    this.release(run_id);
                }
                Err(e) => {
                    tracing::error!(run_id = %run_id, error = %e, "run failed");
                    this.fail(run_id, e.to_string());
                }
            }
            // After the match, because two of the three arms write the run's last log line and a
            // follower is entitled to it. See `close_stream` for what stays behind.
            this.close_stream(run_id);
        });
    }

    /// [`Self::launch`] for the cheap tier. Same arms, same reasons — see it.
    fn launch_task(
        &self,
        run_id: RunId,
        task: offload_core::TaskWork,
        cancel_rx: mpsc::Receiver<Halt>,
    ) {
        let this = self.clone();
        tokio::spawn(async move {
            match this.drive_task(run_id, task, cancel_rx).await {
                Ok(()) => {}
                Err(e @ SubmitError::LostTheRun { .. }) => {
                    tracing::warn!(run_id = %run_id, error = %e, "this leg lost the run");
                    this.release(run_id);
                }
                Err(e) => {
                    tracing::error!(run_id = %run_id, error = %e, "task failed");
                    this.fail(run_id, e.to_string());
                }
            }
            this.close_stream(run_id);
        });
    }

    /// Start the nominated program, put its output in the run's log, and settle the run.
    ///
    /// [`Self::drive`] with everything an agent needs and a task does not taken out: no
    /// workspace hold, because there is no checkout to race over; no `Start`, because there is
    /// no conversation to resume; no turn accounting or checkpoint, because a task has no turn
    /// boundary and so ADR-0004's only-safe-place question never arises.
    ///
    /// **What is deliberately kept identical is the fencing.** A task committing nothing is not
    /// a reason to relax it — a nominated program can send mail, restart a service or post to a
    /// webhook, and two of those at once is the same harm double execution always was. So: the
    /// epoch is checked against the row immediately before the spawn, the process group is
    /// written down before anything else can go wrong, and a `started` transition that will not
    /// go **stops the process**.
    async fn drive_task(
        &self,
        run_id: RunId,
        task: offload_core::TaskWork,
        mut cancel_rx: mpsc::Receiver<Halt>,
    ) -> Result<(), SubmitError> {
        let epoch = lock(&self.live)
            .get(&run_id)
            .map(|l| l.epoch)
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;

        // Read from the row rather than from anything this task remembered, and immediately
        // before the spawn: this is the check that stops two of the owner's programs running
        // at once on one run.
        match self.run(run_id) {
            Some(run) => run.fence(self.node_id, epoch).map_err(|e| {
                self.note_refused(run_id, offload_core::Attempt::Start);
                SubmitError::LostTheRun {
                    run: run_id.short(),
                    reason: e.to_string(),
                }
            })?,
            None => return Err(SubmitError::NoSuchRun(run_id.to_string())),
        }

        // `Task`, not `Agent`. The variant exists for exactly this — it was added when the first
        // walk of the cheap tier prefixed a task's refusal with the word "agent" — and nothing
        // ever constructed it, so the sentence it was written to prevent was still what a task's
        // failure said: `agent: the program for task 'nightly' was not found`, in `offload ps`,
        // `offload logs` and `offload explain`. A variant with no constructor is a fix that did
        // not land.
        let mut handle = crate::task::spawn_task(&self.config, &task.service, &task.args, run_id)
            .await
            .map_err(|e| SubmitError::Task(e.to_string()))?;

        // Written down before anything else can go wrong, for the agent's reason: the handle is
        // in memory, so a `kill -9` of this daemon would otherwise leave the program running on
        // a machine with nothing left to stop it (`crate::leftovers`).
        if let Some(pid) = handle.process_group() {
            crate::leftovers::remember(&self.config.state_dir, run_id, pid, "");
        }

        let taken = match self
            .store
            .update_run(run_id, |run| run.started(self.node_id, epoch, now()))?
        {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(e.to_string()),
            None => Err("the run is no longer in this node's registry".to_string()),
        };
        if let Err(reason) = taken {
            // The same argument `drive` makes: ignoring this would leave a program running for
            // a run this leg no longer holds, which is the double execution the fence exists to
            // prevent, arrived at through the guard.
            handle.stop();
            return Err(SubmitError::LostTheRun {
                run: run_id.short(),
                reason,
            });
        }

        self.append(
            run_id,
            LogKind::TaskStarted {
                service: task.service.to_string(),
            },
        );
        // **This leg is the one writing this run's log, and for a task nothing else says so.**
        // `log_source` answers "which machine serves this run's log" from `RunProgress::by`, on
        // the sound grounds that the leg which produced the numbers is by construction the leg
        // that wrote the log — and a task produces no numbers, so the stamp was never written
        // and the fallback resolved to the run's *arbiter*. When that was the asking node it
        // printed nothing at all: `offload logs` exit 0, no output, on the very node the task
        // was submitted from. That is the silence session sixty-two removed for an agent run,
        // back for the cheap tier, because the fact it turned on was an agent's side effect.
        // Written here rather than at the terminal write: this is the moment the log starts, so
        // a task killed mid-output is still findable.
        self.note_log_leg(run_id);

        // A task's log *is* its output (ADR-0019 §2) — no new plane, and `offload logs` already
        // serves this from the holder.
        let halted = loop {
            tokio::select! {
                biased;
                asked = cancel_rx.recv() => {
                    handle.stop();
                    break Some(asked.unwrap_or(Halt::Superseded));
                }
                line = handle.next_line() => match line {
                    Some(text) => self.append(run_id, LogKind::Text { text }),
                    None => break None,
                },
            }
        };

        let outcome = handle.outcome().await;
        match halted {
            Some(Halt::Cancelled { by }) => {
                self.finish_cancelled(run_id, &by);
                return Ok(());
            }
            // Somebody else's record is coming; there is nothing true this node can add.
            Some(Halt::Superseded) => return Ok(()),
            Some(Halt::Revoked) => {
                self.fail(
                    run_id,
                    "this node was revoked from its fleet, so it stopped running it".to_string(),
                );
                return Ok(());
            }
            None => {}
        }

        match outcome {
            crate::task::TaskOutcome::Succeeded => self.complete_task(run_id, epoch),
            // The status rather than a bare "it failed": every user-facing question here is
            // *why did that not happen*, and `exit status 2` is the whole of the answer a
            // nominated program gives.
            crate::task::TaskOutcome::Failed { status } => self.fail(run_id, status),
        }
        Ok(())
    }

    /// The terminal write for a task that exited zero.
    ///
    /// `update_run` rather than a direct transition, and the epoch it was started under rather
    /// than whatever the row says now — the same discipline the agent's `Finished` arm uses,
    /// for the same reason: a leg that lost the run in its last moment must not write the run's
    /// conclusion.
    fn complete_task(&self, run_id: RunId, epoch: Epoch) {
        let settled = self
            .store
            .update_run(run_id, |run| run.complete(self.node_id, epoch, now()));
        match settled {
            Ok(Some(Err(e))) => {
                tracing::warn!(run_id = %run_id, error = %e, "this leg could not conclude its task");
                return;
            }
            Err(e) => {
                tracing::error!(run_id = %run_id, error = %e, "could not write a task's result");
                return;
            }
            Ok(None) | Ok(Some(Ok(()))) => {}
        }
        self.append(
            run_id,
            LogKind::Finished {
                success: true,
                // Not zeros standing in for an agent's numbers — `work` is what says these
                // three have no referent, and it is what keeps *"finished after 0 turn(s),
                // $0.0000"* off somebody's phone (ADR-0019 §2).
                turns: 0,
                denials: 0,
                cost_micro_usd: 0,
                work: offload_core::WorkKind::Task,
                result: None,
            },
        );
    }

    /// Prepare the workspace, start the agent, pump its events until it stops.
    async fn drive(
        &self,
        run_id: RunId,
        source: RepoSource,
        git_ref: Option<String>,
        mut cancel_rx: mpsc::Receiver<Halt>,
        start: Start,
    ) -> Result<(), SubmitError> {
        // Exclusive hold on this run's checkout for as long as it is being built, and no
        // longer. The checkout sweep and `offload rm` both tear one down with a subprocess, and
        // a rebuild that starts inside that subprocess adopts a directory that is deleted out
        // from under it — measured at 10 of 60 on a forced pair, and every single pass that
        // started within 2ms of the removal (ADR-0052). Dropped at the end of this block: from
        // there on the run is in `live` with an agent handle, which every teardown door already
        // asks about.
        // Before the hold, because acquiring an archive is a fetch and an unpack — seconds to
        // minutes on a large one — and holding the checkout lock across it would block the
        // sweep and `offload rm` for the whole transfer. Nothing here touches the checkout.
        self.ensure_archive(run_id, &source).await?;

        let held = self.workspaces.hold(run_id).await;
        let workspace = match &start {
            Start::Fresh => held.prepare(&source, git_ref.as_deref(), None).await?,
            Start::Resume {
                checkpoint,
                session,
                ..
            } => {
                let (workspace, restored) =
                    self.restore_workspace(&held, &source, checkpoint).await?;
                self.install_transcript(&workspace, session, checkpoint, &restored)?;
                self.append(
                    run_id,
                    LogKind::Resumed {
                        from_turn: checkpoint.turns,
                        session: session.0.clone(),
                        workspace: restored.how,
                    },
                );
                workspace
            }
            Start::Continue { checkpoint, parent } => {
                let (workspace, restored) =
                    self.restore_workspace(&held, &source, checkpoint).await?;
                if let (offload_core::ContinueMode::Session, Some(session)) =
                    (parent.mode, checkpoint.session_id.clone())
                {
                    self.install_transcript(
                        &workspace,
                        &SessionId(session),
                        checkpoint,
                        &restored,
                    )?;
                }
                let mut missing = Vec::new();
                if parent.closing_message.is_none() {
                    missing.push(
                        "the previous run ended with no closing message; only its workspace and \
                         transcript were handed over"
                            .to_string(),
                    );
                }
                if parent.transcript.is_none() {
                    missing.push(
                        "the previous run's transcript could not be found on its node; only its \
                         closing message and its workspace were handed over"
                            .to_string(),
                    );
                }
                self.append(
                    run_id,
                    LogKind::Continued {
                        parent: parent.run.to_string(),
                        mode: parent.mode,
                        base: restored.how,
                        transcript_bytes: parent
                            .transcript
                            .and_then(|hash| self.store.blob_size(hash)),
                        missing,
                    },
                );
                workspace
            }
        };
        drop(held);

        self.append(
            run_id,
            LogKind::WorkspaceReady {
                path: workspace.path.display().to_string(),
                branch: workspace.branch.clone(),
                base: workspace.base_commit[..8.min(workspace.base_commit.len())].to_string(),
            },
        );

        // The repo's own allowlist, read from the checked-out worktree rather than the
        // source repo: what matters is what this run's ref declares. Capped to scoped
        // grants inside `load_allowlist` — a repo may say "run my tests", not "give me sh".
        let repo_allow = if self.config.agent.trust_repo_allowlist {
            match crate::repo_config::load_allowlist(&workspace.path) {
                Ok(allow) => allow,
                Err(e) => {
                    // Refusing the run outright would be harsh, but silently ignoring the
                    // file would leave the agent denied with no explanation of why the
                    // allowlist the repo declared did not apply.
                    tracing::warn!(run_id = %run_id, error = %e, "ignoring repo allowlist");
                    self.append(
                        run_id,
                        LogKind::Failed {
                            reason: format!("repo allowlist ignored: {e}"),
                        },
                    );
                    ToolAllowlist::default()
                }
            }
        } else {
            ToolAllowlist::default()
        };

        let mut run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;

        // A continuation's parent transcript, wherever this leg starts — first start or after a
        // migration, since the worktree is rebuilt from blobs either way (ADR-0064 §3).
        if let Some(parent) = &run.spec.parent {
            self.materialise_parent_transcript(run_id, &source, &workspace, parent)
                .await?;
        }

        // What this node's copy of the run's grant comes to (ADR-0011): the servers, and
        // permission to call them. Resolved here rather than at submit because the tool pattern
        // is spelled with a node's own name for its own resource, so only the holder knows it —
        // which is the same reason the grant travels as a *service*.
        let reachable = self
            .peers
            .get()
            .map(|peers| peers.resources_elsewhere())
            .unwrap_or_default();
        let granted =
            crate::resource::granted(&self.config, &run.spec.resources, &reachable, run_id);
        // Written to a file rather than passed as an argument: a resource's configuration can
        // carry the credential for the service it reaches, and a command line is readable by any
        // local user. Failing to write it is failing the run — the alternative is a run that
        // starts believing it was granted something and finds nothing there.
        let mcp = match granted.mcp {
            Some(json) => Some(
                crate::resource::write_config(&self.config.state_dir, run_id, &json).map_err(
                    |e| SubmitError::Agent(format!("writing the run's mcp config: {e}")),
                )?,
            ),
            None => None,
        };
        // The two grants this leg adds to the spec, applied to the row rather than to the copy
        // read above it: everything between the two is filesystem and config work, and the row
        // has a heartbeat and a pump writing to it throughout.
        run = self
            .store
            .update_run(run_id, |run| {
                if let Some(work) = run.spec.agent_mut() {
                    work.allow = work
                        .allow
                        .merged_with(&repo_allow)
                        .merged_with(&granted.allow);
                }
                run.clone()
            })?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;

        let work = run.spec.agent().ok_or(SubmitError::UnsupportedWork {
            kind: run.spec.work.kind().name(),
        })?;
        let (prompt, model, permission, allow) = (
            work.prompt.clone(),
            work.model.clone(),
            work.permission_mode,
            work.allow.clone(),
        );
        let epoch = lock(&self.live).get(&run_id).map_or(run.epoch, |l| l.epoch);

        for (pattern, risk) in allow.risky() {
            tracing::warn!(
                run_id = %run_id,
                %pattern,
                ?risk,
                "allowlist entry grants more than it appears to"
            );
        }

        // A run that may ask carries a hook; one that may not carries nothing, which is the
        // behaviour that has always existed (ADR-0017).
        let ask = run
            .spec
            .agent()
            .is_some_and(|work| work.ask.enabled())
            .then(|| self.ask_hook(&run))
            .flatten();

        // A handoff's prompt is the continuer's, followed by what came before (ADR-0064 §3) —
        // composed here, at the one place the agent is told anything, from the two strings the
        // spec carries.
        let prompt = match (&start, &run.spec.parent) {
            (Start::Continue { .. }, Some(parent))
                if parent.mode == offload_core::ContinueMode::Handoff =>
            {
                handoff_prompt(
                    &prompt,
                    parent.run,
                    &parent.parent_prompt,
                    parent.closing_message.as_deref(),
                    parent.transcript.is_some(),
                )
            }
            _ => prompt,
        };
        let (session, resume) = match &start {
            Start::Fresh => (SessionId::generate(), Resume::Fresh),
            // The parent's conversation, forked: its file stays as it ended, and this run's
            // turns go into a session of its own (ADR-0064 §3, the migration path verbatim).
            Start::Continue { checkpoint, parent }
                if parent.mode == offload_core::ContinueMode::Session =>
            {
                match checkpoint.session_id.clone() {
                    Some(session) => (
                        SessionId(session),
                        Resume::Continue {
                            prompt: prompt.clone(),
                            fork: true,
                        },
                    ),
                    None => (SessionId::generate(), Resume::Fresh),
                }
            }
            Start::Continue { .. } => (SessionId::generate(), Resume::Fresh),
            Start::Resume {
                session, prompt, ..
            } => (
                session.clone(),
                // Not forked, unlike a migration: nothing else is alive to write into this
                // transcript — `resume` refuses a run this daemon is still driving, and a
                // previous daemon's agent died with it. Keeping the session id means the
                // conversation stays one thread across any number of resumes.
                Resume::Continue {
                    prompt: prompt.clone(),
                    fork: false,
                },
            ),
        };

        let req = SpawnRequest {
            run: run_id,
            session,
            cwd: workspace.path.clone(),
            prompt,
            model,
            permission_mode: permission,
            allow,
            resume,
            // Exactly the resources this run was granted, and no others — the grant is on the
            // spec, so a migrated run is handed the new node's copy of what it was promised
            // rather than the old node's programs (ADR-0002: nothing about a resource travels).
            mcp,
            // The hook inherits this, which is how a question knows which run and which *leg* of
            // it is asking. Nothing secret travels here (ADR-0002): a run id, an epoch, and the
            // path of a socket on this machine.
            env: ask
                .as_ref()
                .map(|_| {
                    vec![
                        (ASK_RUN_ENV.to_string(), run_id.to_string()),
                        (ASK_EPOCH_ENV.to_string(), epoch.0.to_string()),
                        (
                            ASK_SOCKET_ENV.to_string(),
                            self.config.socket_path().display().to_string(),
                        ),
                    ]
                })
                .unwrap_or_default(),
            ask,
        };

        // **Before** the agent exists, against the store rather than against what this task
        // remembered. Everything above here takes time somebody else can act in: a cold clone can
        // run for minutes, and restoring a workspace fetches blobs from peers — which is exactly
        // the sort of slowness that gets a node concluded dead and its run reassigned. The check
        // below used to be the only one, and it is *after* the spawn.
        match self.store.load_run(run_id)? {
            Some(run) => run.fence(self.node_id, epoch).map_err(|e| {
                // The most important row in the audit log: past this point there would have been
                // two agents on one repository.
                self.note_refused(run_id, offload_core::Attempt::Start);
                SubmitError::LostTheRun {
                    run: run_id.short(),
                    reason: e.to_string(),
                }
            })?,
            None => return Err(SubmitError::NoSuchRun(run_id.to_string())),
        }

        let mut handle = self
            .agent
            .spawn(&req)
            .await
            .map_err(|e| SubmitError::Agent(e.to_string()))?;

        // Write the agent's process group down before anything else can go wrong. The handle
        // holding it is in memory, so a `kill -9` of this daemon would otherwise leave the agent
        // running to completion on a machine with nothing left to stop it (`crate::leftovers`).
        if let Some(pid) = handle.process_group() {
            crate::leftovers::remember(&self.config.state_dir, run_id, pid, handle.session_id());
        }

        // The agent is up; move the run out of Assigned — and if it will not go, **stop it**.
        //
        // This used to discard the failure, which made the fence decorative on the one path where
        // it matters most: the transition is the epoch check, so ignoring it meant an agent that
        // had lost its run kept running, on the same repo, committing, while somebody else
        // resumed the same conversation elsewhere. Double execution, arrived at through the
        // guard that exists to prevent it. The window is small now that the check above closed
        // most of it, and small is not a reason to leave an agent going.
        let taken = match self
            .store
            .update_run(run_id, |run| run.started(self.node_id, epoch, now()))?
        {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(e.to_string()),
            None => Err("the run is no longer in this node's registry".to_string()),
        };
        if taken.is_err() {
            self.note_refused(run_id, offload_core::Attempt::Start);
        }
        if let Err(reason) = taken {
            tracing::warn!(
                run_id = %run_id,
                epoch = epoch.0,
                %reason,
                "this run moved on while its agent was starting; stopping the agent rather than \
                 letting two of them work on one repo"
            );
            let grace = std::time::Duration::from_secs(self.config.agent.cancel_grace_secs);
            let _ = handle.cancel(grace).await;
            self.release(run_id);
            crate::resource::forget_config(&self.config.state_dir, run_id);
            crate::leftovers::forget(&self.config.state_dir, run_id);
            return Err(SubmitError::LostTheRun {
                run: run_id.short(),
                reason,
            });
        }

        // A resumed agent numbers its turns from one, because it is a new process. The
        // run's turn count must not go backwards: `ps` would show it losing progress, and
        // a checkpoint recorded at "turn 1" after one taken at turn 6 would claim less
        // work than the checkpoint it replaces.
        let turn_offset = match &start {
            // A continuation is a new run, so its turns count from one like any other.
            Start::Fresh | Start::Continue { .. } => 0,
            Start::Resume { checkpoint, .. } => checkpoint.turns,
        };
        // Where this leg's spend starts from (ADR-0067 §1): nothing for a fresh start, and the
        // restored transcript's tokens for a resume — counted now, from the checkpoint itself,
        // rather than from whatever the gossip happens to have told this row by the first capture.
        let base = match &start {
            Start::Fresh | Start::Continue { .. } => offload_core::TokenUse::default(),
            Start::Resume { checkpoint, .. } => self
                .store
                .get_blob(checkpoint.transcript)
                .map(|bytes| offload_agent::usage::from_transcript(&bytes))
                .unwrap_or_default(),
        };
        if let Some((by, epoch)) = self.writing_leg(run_id) {
            let _ = self
                .store
                .update_stats(run_id, |stats| begin_leg(stats, by, epoch, base).by);
        }

        let (stop, session) = self
            .pump(
                run_id,
                &workspace,
                &req.session,
                turn_offset,
                &mut handle,
                &mut cancel_rx,
            )
            .await;

        if stop != Stop::Finished {
            // Both remaining cases end the agent process: a cancel outright, and a released
            // checkpoint because the run now belongs to whoever picks it up next. Leaving
            // it alive would keep writing into a worktree somebody else may be restoring.
            let grace = std::time::Duration::from_secs(self.config.agent.cancel_grace_secs);
            let _ = handle.cancel(grace).await;
        }
        // Whatever the reason, this daemon is no longer driving an agent for this run, and
        // the handle has to say so: it is what `cancel` and `resume` read to decide whether
        // anything is still holding the worktree.
        self.release(run_id);
        crate::resource::forget_config(&self.config.state_dir, run_id);
        crate::leftovers::forget(&self.config.state_dir, run_id);

        // Did this leg still believe the run was its own? That is what decides, below, whether a
        // describe it turns out not to be allowed is a *refusal* or simply the ordinary end of a
        // leg that had already been told otherwise.
        let believed_it_held = matches!(
            stop,
            Stop::Finished
                | Stop::Halted(Halt::Cancelled { .. })
                | Stop::Halted(Halt::Revoked)
                | Stop::LimitReached { .. }
        );

        match stop {
            Stop::Halted(Halt::Cancelled { by }) => self.finish_cancelled(run_id, &by),
            // `Failed`, and deliberately not `Completed`: the agent was stopped with the task
            // in whatever state the last turn left it, and `Completed` is terminal *and*
            // successful — an abandoned run that reads as a success is the failure the resume
            // nudge exists to avoid, and it would be this one every time.
            //
            // Not `Cancelled` either, whose `by` names the node an operator typed `offload
            // cancel` at. There is no such node here, and inventing one would put a lie in the
            // one field that variant exists to carry.
            //
            // `fail` is the right writer rather than a direct transition: it asks `holds`
            // first, so a leg that reached the limit in the same moment it lost the run writes
            // nothing — the limit is this leg's observation, and a terminal write it is not
            // entitled to make is the unfenced write this file is most dangerous for.
            Stop::LimitReached { limit } => {
                self.fail(run_id, format!("it reached its turn limit of {limit}"));
            }
            // The agent is down and the record is somebody else's. `record_run` has already
            // saved their copy and warned; there is nothing true this node can add to the log.
            Stop::Halted(Halt::Superseded) => {}
            // The opposite case, and the reason it is a separate variant: nobody else's record
            // is coming. This node is out of its fleet and can hear nothing, so the only honest
            // thing left on this machine is what it writes itself (ADR-0044). `fail` asks
            // `holds` first, exactly as the turn-limit arm does.
            Stop::Halted(Halt::Revoked) => {
                self.fail(
                    run_id,
                    "this node was revoked from its fleet, so it stopped running it".to_string(),
                );
            }
            // Nothing else to do: `checkpoint` already moved the run back to `Pending`
            // under the epoch it held.
            Stop::Checkpointed => {}
            Stop::Finished => self.fail_if_unfinished(run_id),
        }

        // What *this* checkout holds — and only while the run is still this node's to describe.
        // These numbers gossip, and `accept_progress` breaks a tie on the author's clock, so the
        // last write wins the fleet: a leg that had just lost the run would publish its own stale
        // worktree as the run's, over the new holder's, in the column somebody reads to find out
        // whether there is uncommitted work. The same arithmetic that had `offload rm` telling
        // the fleet a checkout on another machine was gone.
        //
        // Not being allowed to describe it is a *refusal* only when this leg thought it still
        // could. `Refused` rows are what somebody reads when they suspect two agents on one
        // repository, so they have to stay rare and real — and every graceful drain wrote one:
        // the departing leg is superseded a moment after handing its run over, `describes` goes
        // false, and the audit log reported a fence on the ordinary path this product is named
        // for. A superseded leg was *told*, and `AuditEvent::Superseded` is that fact already
        // written down; a checkpointed one handed the run back itself. Neither attempted
        // anything it believed it was entitled to, which is the whole of what `Refused` means.
        if self.describes(run_id) {
            self.refresh_workspace_summary(run_id, &workspace).await;
            self.note_tokens(run_id, &workspace, &session);
        } else {
            // A leg that lost may not say where the run is, and it still spent what it spent
            // (ADR-0067): its own entry, from its own transcript, and nothing else.
            self.note_leg_tokens(run_id, epoch, &workspace, &session);
            if believed_it_held {
                self.note_refused(run_id, offload_core::Attempt::Describe);
            }
        }
        Ok(())
    }

    /// Consume the agent's event stream until it ends, is cancelled, or is checkpointed.
    async fn pump(
        &self,
        run_id: RunId,
        workspace: &Workspace,
        requested_session: &SessionId,
        turn_offset: u32,
        handle: &mut offload_agent::claude::RunHandle,
        cancel_rx: &mut mpsc::Receiver<Halt>,
    ) -> (Stop, String) {
        let mut session = requested_session.0.clone();
        let mut agent_version = String::new();

        loop {
            tokio::select! {
                // Biased so a cancel that arrives alongside a burst of events is acted on
                // promptly rather than after the burst drains.
                biased;
                asked = cancel_rx.recv() => return (Stop::Halted(
                    // The sender is dropped only by `release`, which runs after the pump ends —
                    // so a `None` here is a channel closed by something that never asked for
                    // anything, and closing the run's record on the strength of it would be
                    // inventing a decision. Stop the agent, write nothing.
                    asked.unwrap_or(Halt::Superseded),
                ), session),
                event = handle.next_event() => {
                    let Some(event) = event else { return (Stop::Finished, session) };

                    if let AgentEvent::Started { session_id, agent_version: version, .. } = &event {
                        // What the agent says it opened, which is not always what we asked
                        // for: `--fork-session` mints a new id, and the transcript is
                        // written under that one.
                        session.clone_from(session_id);
                        agent_version.clone_from(version);
                        // And the worktree is demonstrably no longer being prepared, which is
                        // what it had been claiming since `launch`. A cold clone runs for
                        // minutes, so "preparing" was true while it lasted and false the moment
                        // the agent came up.
                        self.refresh_workspace_summary(run_id, workspace).await;
                    }
                    // Rewritten here and nowhere else, so everything downstream — stats,
                    // the event log, checkpoints, the cadence — counts turns the same way.
                    let event = accumulate_turns(event, turn_offset);
                    let boundary = match &event {
                        AgentEvent::TurnBoundary { turn } => Some(*turn),
                        _ => None,
                    };
                    self.record_event(run_id, event);

                    let Some(turn) = boundary else { continue };

                    // What the checkout holds now, on **every** boundary rather than only the
                    // ones that check point. `RunProgress::workspace` is documented as "the
                    // holder's summary of the run's worktree — 2 modified, 1 new", and it was
                    // written twice in a run's life: `preparing` at launch, and the truth once
                    // the leg had ended. So `ps` said `preparing` for eight hours, in the column
                    // beside the one saying whether that work is replicated, and it gossiped —
                    // the whole fleet reporting a worktree as half-built while an agent worked
                    // in it. Tying it to the checkpoint instead would be two turns stale under
                    // `every_turns = 3` and permanently wrong under `every_turns = 0`, which is
                    // the bug unchanged for anyone who turned automatic captures off. One `git
                    // status` per turn, against minutes of model time.
                    self.refresh_workspace_summary(run_id, workspace).await;

                    // The budget, asked before the cadence and before the operator's own
                    // request, because reaching it settles both: this run is ending here, so
                    // the capture is not optional and a pending release is moot — releasing it
                    // would put a spent run back in the pool for somebody else to continue.
                    //
                    // Captured first and unconditionally, whatever `every_turns` says. A run
                    // stopped by its limit is exactly the run somebody wants to look at and
                    // continue by hand, and under `every_turns = 0` the alternative is ending
                    // it on the last checkpoint it happened to take, or on none at all.
                    let limit = self
                        .run(run_id)
                        .and_then(|run| run.spec.turn_limit_reached(turn));
                    if let Some(limit) = limit {
                        tracing::info!(
                            run_id = %run_id,
                            turn,
                            limit,
                            "the run has taken the turns it was given; stopping it here"
                        );
                        // **Stop the agent before capturing, which no other capture may do.**
                        //
                        // The transcript is the agent's own file and it is written *behind* its
                        // event stream by an amount it does not promise, so every capture taken
                        // while the process is alive races the flush — and a capture that lands
                        // on the empty side is refused rather than stored, because resuming from
                        // it loses the agent's half of the conversation. Every other capture can
                        // afford that race: `CaptureFailed`, and the next boundary tries again.
                        //
                        // This boundary is the last one there will ever be. The retry that makes
                        // the refusal harmless everywhere else does not exist here, so the race
                        // decides whether the run somebody capped ends with a checkpoint or with
                        // none — and `note_tokens` already says which way it leans: "the run it
                        // is emptiest for is the short one, which is exactly the run somebody
                        // caps". Measured on `--max-turns 1` with a real agent: **two of three
                        // capped runs ended with no `checkpoint` line in `offload explain` at
                        // all** and a dash under SAFE in `ps`, which is precisely the outcome
                        // capturing unconditionally here (ADR-0039 §4) exists to prevent.
                        //
                        // **The order is wait, then stop, then capture, and the first version of
                        // this fix had it wrong.** That version stopped the agent and then
                        // captured, reasoning that the file is final once the process is gone —
                        // `note_tokens`' own premise, borrowed. It is true and it is not enough:
                        // a `SIGTERM` landing before the agent has flushed the turn it just
                        // spoke loses that message *outright* rather than writing it on the way
                        // out. Measured — three failing transcripts, each exactly 24,188 bytes,
                        // still holding zero assistant rows long after the processes that owned
                        // them had exited. Final, and empty. So the wait has to come before the
                        // stop: after it, nothing more is ever coming.
                        //
                        // Stopping before capturing is still right, and for its own reason: the
                        // patch is then taken from a worktree nobody is writing into.
                        //
                        // `cancel` is idempotent — it clears the child handle — so the caller's
                        // own cancel after `pump` returns stays correct and does nothing twice.
                        self.await_transcript(run_id, workspace, &session, turn).await;
                        let grace =
                            std::time::Duration::from_secs(self.config.agent.cancel_grace_secs);
                        if let Err(e) = handle.cancel(grace).await {
                            // Not fatal to the capture: the process may already be gone, and the
                            // read below is what actually decides. Worth saying because a stop
                            // that failed is why a transcript might still be moving under it.
                            tracing::warn!(
                                run_id = %run_id,
                                error = %e,
                                "could not stop the agent before capturing at the turn limit"
                            );
                        }
                        if let Err(e) = self
                            .checkpoint(run_id, workspace, &session, &agent_version, turn, None)
                            .await
                        {
                            // The limit stops the run either way. A capture that failed loses
                            // the way *back* into the conversation, not the decision to stop —
                            // and saying so is better than carrying on to spend a turn the
                            // operator did not authorise because the save did not work.
                            tracing::error!(
                                run_id = %run_id,
                                error = %e,
                                "could not capture the run that reached its turn limit"
                            );
                            // `run_continues: false`, which is the whole of what this path got
                            // wrong in its report: the line under a failed capture said "the
                            // run continues; the next turn boundary tries again" one line above
                            // the line saying the run had ended. True of the caller this variant
                            // was written for, and false of this one from the day it was added.
                            self.append(
                                run_id,
                                LogKind::CaptureFailed {
                                    turn,
                                    reason: e.to_string(),
                                    run_continues: false,
                                },
                            );
                        }
                        return (Stop::LimitReached { limit }, session);
                    }

                    let release = self.checkpoint_requested(run_id);
                    if release.is_none() && !self.cadence_due(turn) {
                        continue;
                    }

                    match self
                        .checkpoint(run_id, workspace, &session, &agent_version, turn, release)
                        .await
                    {
                        Ok(()) if release.is_some() => {
                            self.clear_checkpoint_request(run_id);
                            return (Stop::Checkpointed, session);
                        }
                        Ok(()) => {}
                        Err(e) => {
                            // A failed capture is not a failed run: the agent is unharmed
                            // and the next boundary will try again. Releasing the run on
                            // one would be the dangerous move — handing over nothing.
                            tracing::error!(run_id = %run_id, error = %e, "checkpoint failed");
                            // `CaptureFailed`, not `Failed`: the run has not failed, and a
                            // log kind named after a state must not be borrowed for an
                            // operation inside it. Everything downstream believes it —
                            // `is_terminal` hung up every follower mid-run, and the delivery
                            // plane told somebody their run had failed just before telling
                            // them it had finished.
                            self.append(
                                run_id,
                                LogKind::CaptureFailed {
                                    turn,
                                    reason: e.to_string(),
                                    run_continues: true,
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    /// Read what the conversation consumed and record it, once the agent is down (ADR-0040).
    ///
    /// **Why this exists as well as the read inside `checkpoint`.** The agent writes its
    /// transcript *behind* its event stream, and by an amount it does not promise. Measured on
    /// claude 2.1.251: a checkpoint taken at the turn-1 boundary captured 17KB of transcript
    /// containing `queue-operation`, `user`, `attachment`, `atis-latch` and `ai-title` rows and
    /// **no assistant rows at all**, while the same file on disk afterwards held three. So the
    /// per-checkpoint read is always a turn or so stale and can be empty outright — and the run
    /// it is emptiest for is the short one, which is exactly the run somebody caps.
    ///
    /// Once the agent's process is gone the file is final, so this is the read that is complete.
    /// Called from inside the `describes` branch rather than beside it, because it asks the same
    /// question that branch does: these numbers gossip, and a leg that has lost the run is not
    /// entitled to state them however true they are of the checkout it was working in.
    ///
    /// Silent on every failure. It runs after a run has already ended — a missing transcript
    /// means a leg that never got that far, and a report has no business turning that into an
    /// error on a path that has nothing left to fail.
    fn note_tokens(&self, run_id: RunId, workspace: &Workspace, session: &str) {
        let Ok(path) = transcript::find(&self.home, &workspace.path, session) else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let tokens = offload_agent::usage::from_transcript(&bytes);
        // Unknown is not none: a parse that found nothing must not overwrite a number an
        // earlier checkpoint did find.
        if tokens.is_empty() {
            return;
        }
        self.update_stats(run_id, |stats| stats.tokens = tokens);
    }

    /// What a leg that lost the run spent, from its transcript, into its own entry only (ADR-0067).
    ///
    /// The case the checkpoint path cannot reach, measured on two daemons: a leg told it was
    /// reassigned stops its agent before any turn boundary, so no capture ever reads its
    /// transcript, and the tokens its agent had already spent in that turn were counted nowhere.
    /// The agent is down by now, so the file is final (see [`Self::note_tokens`]).
    ///
    /// **Not** through `update_stats`, which stamps the position: this leg is not entitled to say
    /// where the run is, only what it itself consumed. So the store is written directly, the
    /// position and its stamp are left exactly as they are, and the entry only ever grows.
    fn note_leg_tokens(
        &self,
        run_id: RunId,
        epoch: offload_core::run::Epoch,
        workspace: &Workspace,
        session: &str,
    ) {
        let Ok(path) = transcript::find(&self.home, &workspace.path, session) else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let tokens = offload_agent::usage::from_transcript(&bytes);
        if tokens.is_empty() {
            return;
        }
        let me = self.node_id;
        let _ = self.store.update_stats(run_id, |stats| {
            // The entry its start created, and only that one (see `update_stats`).
            if let Some(entry) = stats
                .legs
                .iter_mut()
                .find(|l| l.by == me && l.epoch == epoch)
            {
                entry.tokens = entry.tokens.max(tokens);
            }
        });
    }

    /// Wait, briefly and boundedly, for the agent to write the turn it has just finished.
    ///
    /// **Only the turn limit calls this, and only because it is the one capture with no retry.**
    /// Every other failed capture is answered at the next boundary, which is why the guard that
    /// refuses an empty transcript could say there was "nothing here to wait on that would be
    /// honest". At the limit there is no next boundary, so a wait is the only thing left that can
    /// help — and it is not the expiry ADR-0035 §4 rejects, because it decides nothing on
    /// anybody's behalf. It waits for a file the agent is in the middle of writing.
    ///
    /// **Measured, and the failure is binary rather than gradual.** Seven `--max-turns 1` runs on
    /// claude 2.1.251 / haiku 4.5: every transcript that had not caught up was *exactly* 24,188
    /// bytes with zero assistant rows — the session preamble, three times, byte-identical — and
    /// every one that had was 29.8–31.4KB with two or three. So there is no partial state to
    /// read: either the agent has flushed the message it just spoke or it has not.
    ///
    /// **It is not a lag that exit resolves**, which is what the first version of this fix
    /// assumed. Those 24,188-byte files still hold zero assistant rows now, long after the
    /// processes that owned them are gone — a `SIGTERM` landing inside the window loses the
    /// message outright rather than flushing it on the way out. Which is why the wait comes
    /// *before* the stop and not after it: once the agent is gone, nothing more is coming.
    ///
    /// Silent, and it never fails anything. A transcript that does not arrive leaves the capture
    /// to refuse itself exactly as it did before — the limit still ends the run (ADR-0039 §4),
    /// and this only changes how often that ends with something to look at.
    async fn await_transcript(
        &self,
        run_id: RunId,
        workspace: &Workspace,
        session: &str,
        turn: u32,
    ) {
        let Ok(path) = transcript::find(&self.home, &workspace.path, session) else {
            return;
        };
        let started = std::time::Instant::now();
        let deadline = started + TRANSCRIPT_FLUSH_WAIT;
        let mut waited = false;
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                if !offload_agent::usage::from_transcript(&bytes).is_empty() {
                    if waited {
                        // With the elapsed time, because it is the number that says whether
                        // `TRANSCRIPT_FLUSH_WAIT` is the right size. Measured over eight capped
                        // runs it never reached a second, and the timeout never fired.
                        tracing::debug!(
                            run_id = %run_id,
                            turn,
                            waited_ms = started.elapsed().as_millis(),
                            "the transcript caught up before the turn limit stopped the agent"
                        );
                    }
                    return;
                }
            }
            if std::time::Instant::now() >= deadline {
                // Said once, and at `warn`, because it is the line that explains a capped run
                // with no checkpoint — the question somebody will be asking of this log.
                tracing::warn!(
                    run_id = %run_id,
                    turn,
                    waited_ms = TRANSCRIPT_FLUSH_WAIT.as_millis(),
                    "the agent never wrote turn {turn} to its transcript; capturing anyway"
                );
                return;
            }
            waited = true;
            tokio::time::sleep(TRANSCRIPT_POLL).await;
        }
    }

    /// Is this turn one the cadence asks for? (`every_turns = 0` disables automatic ones.)
    fn cadence_due(&self, turn: u32) -> bool {
        let every = self.config.checkpoint.every_turns;
        every > 0 && turn % every == 0
    }

    /// Is the operator waiting for a checkpoint? **Read without clearing.**
    ///
    /// It used to read-and-clear, which lost the request whenever the capture that followed
    /// failed: `offload checkpoint` reported "it will be taken at the next turn boundary", the
    /// capture at that boundary failed, and the run carried on to completion having never
    /// released — the operator asked for something, was told it was coming, and got silence.
    /// Reachable before this by any capture failure and now easy to hit, because a transcript
    /// that has not caught up is refused (measured: exactly this, on the first walk of that
    /// guard). A request is a standing instruction until it is honoured, so it is cleared by
    /// the one arm that honours it — and it does not need clearing anywhere else, because
    /// [`Self::register`] inserts a *fresh* entry for every leg, so nothing a previous one was
    /// asked for can reach this one. Not because the entry goes: it does not (see [`LiveRun`]),
    /// and a request left standing on a leg that has ended is refused by `request_checkpoint`'s
    /// own guard rather than by the flag being clear.
    fn checkpoint_requested(&self, run_id: RunId) -> Option<GivenUp> {
        lock(&self.live)
            .get(&run_id)
            .and_then(|l| l.checkpoint_requested)
    }

    /// Mark the operator's request honoured.
    fn clear_checkpoint_request(&self, run_id: RunId) {
        if let Some(entry) = lock(&self.live).get_mut(&run_id) {
            entry.checkpoint_requested = None;
        }
    }

    /// Capture everything needed to continue this run elsewhere, and record it.
    ///
    /// `release` picks which of the two checkpoints this is: `None` for the routine per-turn
    /// one that keeps the run here, and otherwise the requested one that hands it back to the
    /// pool. Both write the same blobs; only the state transition differs.
    ///
    /// It carries *who* asked rather than merely that somebody did, because the released run is
    /// two different things to the fleet depending on the answer, and this is the last place
    /// that knows (ADR-0042).
    async fn checkpoint(
        &self,
        run_id: RunId,
        workspace: &Workspace,
        session: &str,
        agent_version: &str,
        turn: u32,
        release: Option<GivenUp>,
    ) -> Result<(), SubmitError> {
        let capture =
            offload_workspace::checkpoint::capture(workspace, &self.config.untracked_policy())
                .await?;
        let summary = capture.summary();

        // No transcript means no conversation to resume into, which makes the git side
        // worthless on its own — better to fail the checkpoint loudly than to record one
        // that cannot do its job.
        let path = transcript::find(&self.home, &workspace.path, session)
            .map_err(|e| SubmitError::Agent(e.to_string()))?;
        let transcript_bytes = std::fs::read(&path).map_err(|e| {
            SubmitError::Agent(format!("reading transcript {}: {e}", path.display()))
        })?;

        // Blobs are the one genuinely large write on this path — a bundle can be tens of
        // megabytes — so it goes to a blocking thread rather than stalling the runtime.
        let store = self.store.clone();
        let (bundle_bytes, patch_bytes) = (capture.bundle, capture.patch);
        let stored = tokio::task::spawn_blocking(move || {
            // What the conversation has consumed (ADR-0040), parsed from the bytes already in
            // hand. Here rather than on the runtime because the transcript is the largest text
            // on this path and this is the thread that was going to be blocked writing it
            // anyway — and here rather than at the turn boundary because re-reading a growing
            // transcript every turn is quadratic in a run's length.
            let tokens = offload_agent::usage::from_transcript(&transcript_bytes);
            let transcript = store.put_blob(&transcript_bytes)?;
            let bundle = bundle_bytes.map(|b| store.put_blob(&b)).transpose()?;
            let patch = patch_bytes.map(|p| store.put_blob(&p)).transpose()?;
            Ok::<_, offload_store::StoreError>((transcript, bundle, patch, tokens))
        })
        .await
        .map_err(|e| SubmitError::Agent(format!("blob write task failed: {e}")))??;
        let (transcript, bundle, patch, tokens) = stored;

        // **A transcript with no assistant messages in it is a transcript with no conversation
        // in it**, and the rule two blocks up already covers it: better to fail the checkpoint
        // loudly than to record one that cannot do its job. That check tested only that the file
        // *existed*, which is a weaker thing than it looks, because the agent writes the file
        // behind its own event stream by an amount it does not promise.
        //
        // Measured, and it is a race rather than a fixed lag — two runs checkpointed at the same
        // turn 1 stored 22,953 bytes with the conversation in it and 17,087 bytes with none of
        // it. The failure the second one causes is the worst-shaped one available: resumed, the
        // agent has lost its own half of the conversation, says "No response requested.", makes
        // no tool call, writes nothing, and the run reports **`Completed`**. An abandoned run
        // that reads as a success, arrived at from a new direction. The control run, whose
        // capture had caught up, resumed and wrote the nanosecond timestamp that existed only in
        // the conversation — so the mechanism is right and it is the bytes that were not there.
        //
        // Refused rather than repaired: the agent owns that file and there is nothing here to
        // wait on that would be honest. A failed capture is not a failed run — the next boundary
        // tries again, by which time the flush has caught up — and the one thing it must not do
        // is *release* the run, which the caller already gets right.
        if tokens.is_empty() {
            return Err(SubmitError::Agent(format!(
                "the agent's transcript holds no conversation yet at turn {turn}"
            )));
        }
        // There was a `messages < turn` warning here, and measuring it is what removed it: on a
        // healthy five-turn run it fired at **every** boundary — turn 2 with 1 message, turn 3
        // with 1, turn 4 with 2, turn 5 with 3. Being about a turn behind is not a symptom, it
        // is the normal state of the file, so a warning for it is a line that trains people to
        // ignore the log. Only *nothing* is fatal, which is what the check above tests.
        self.update_stats(run_id, |stats| stats.tokens = tokens);

        let checkpoint = Checkpoint {
            replicas: std::collections::BTreeSet::new(),
            session_id: Some(session.to_string()),
            transcript,
            bundle,
            patch,
            base_commit: workspace.base_commit.clone(),
            turns: turn,
            taken_at: now(),
            agent_version: agent_version.to_string(),
        };

        // Under the store's lock, because everything above this took time: capturing a
        // worktree, reading a transcript, and writing blobs on another thread. A load here and
        // a save two lines further on is a window the pump can finish a run in, and what the
        // save would then write is this run as it stood before it finished.
        let live_epoch = lock(&self.live).get(&run_id).map(|l| l.epoch);
        self.store
            .update_run(run_id, |run| {
                let epoch = live_epoch.unwrap_or(run.epoch);
                if let Some(given_up) = release {
                    run.checkpointed(self.node_id, epoch, checkpoint.clone(), given_up, now())
                } else {
                    run.record_checkpoint(self.node_id, epoch, checkpoint.clone())
                }
            })?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?
            .map_err(|e| SubmitError::Agent(e.to_string()))?;

        // What this node's checkout now holds, for the resume that has to decide whether it
        // is still the current one. After the store, because a marker for a checkpoint that
        // was never recorded would claim a turn nothing can restore.
        self.workspaces.note_turn(run_id, turn);

        tracing::info!(run_id = %run_id, turn, ?release, %summary, "checkpoint recorded");
        self.append(
            run_id,
            LogKind::Checkpointed {
                turn,
                summary,
                released: release.is_some(),
            },
        );
        self.replicate(run_id, checkpoint);
        Ok(())
    }

    /// Rebuild — or simply find — the worktree a checkpoint describes.
    ///
    /// Returns the workspace and a short account of how it got there, because "restored
    /// from a bundle" and "it was still on disk" have very different implications when a
    /// resumed run turns out to be missing something.
    /// Takes the checkout guard rather than the run id: every step below changes this node's
    /// copy of the run's worktree, and the guard is both the exclusion and the id (ADR-0052).
    async fn restore_workspace(
        &self,
        held: &offload_workspace::CheckoutGuard<'_>,
        source: &RepoSource,
        checkpoint: &Checkpoint,
    ) -> Result<(Workspace, Restored), SubmitError> {
        let run_id = held.run();
        let branch = offload_workspace::run_branch(run_id);

        // What is on this node's disk, measured against the checkpoint rather than assumed
        // to be current: a run that left and came back leaves the checkout from the earlier
        // leg behind, and adopting that one resumes the agent before everything the other
        // machine did.
        let mut rescued = None;
        match held.adopt(&branch, &checkpoint.base_commit, checkpoint.turns) {
            offload_workspace::Adoption::Current(workspace) => {
                return Ok((
                    workspace,
                    Restored {
                        how: "adopted in place".to_string(),
                        adopted: true,
                    },
                ));
            }
            offload_workspace::Adoption::Superseded { at_turn } => {
                let what = held.supersede(source).await?;
                match &what {
                    offload_workspace::Rescued::MovedAside(_) => {
                        self.note_rescued(run_id, offload_core::Rescue::Checkout);
                    }
                    // The third door of the three `Reclamation` names, and it belongs with the
                    // other two rather than with the rescue: the disk changed, and what the
                    // reader wants is which decision took it.
                    offload_workspace::Rescued::Redundant => {
                        self.note_reclaimed(run_id, offload_core::Reclamation::Redundant);
                    }
                }
                rescued = Some((at_turn, what));
            }
            offload_workspace::Adoption::Absent => {}
        }

        // The branch outlives the worktree — `offload rm` keeps it precisely so this is
        // possible — so when it is still in the mirror it is the right thing to check out:
        // it may hold commits from a leg that is *ahead* of this checkpoint, and starting
        // from the base would throw them away.
        //
        // **What it is not is a reason to skip the bundle**, which is what this used to do.
        // A branch of that name is there whenever this node has ever run a leg of this run,
        // and a branch that is merely *older* than the checkpoint then suppressed the bundle
        // silently: a run that ran here, migrated, committed elsewhere and came back kept
        // none of what the other machine did. Measured on two daemons — alpha's six commits
        // and the returning checkout shared **only the base commit**, with `offload logs`
        // reporting `re-checked out, patch reapplied` over the top of a bundle that had been
        // fetched and never opened. `restore` decides now, by asking git whether this
        // checkout already contains the bundle's tip, which is the question that was meant.
        let start_ref = if self.workspaces.has_branch(source, &branch).await {
            branch.clone()
        } else {
            checkpoint.base_commit.clone()
        };

        let mut workspace = held
            .prepare(source, Some(&start_ref), Some(&branch))
            .await?;
        // `prepare` reports the ref it checked out as the base. For a resumed run that is
        // wrong: the base is where the *run* started, and every later capture bundles
        // `base..HEAD` against it. Keeping the checkpoint's value is what stops the next
        // checkpoint from silently bundling nothing.
        workspace.base_commit.clone_from(&checkpoint.base_commit);

        let bundle = checkpoint
            .bundle
            .map(|hash| self.store.get_blob(hash))
            .transpose()?;
        let patch = checkpoint
            .patch
            .map(|hash| self.store.get_blob(hash))
            .transpose()?;

        let outcome =
            offload_workspace::checkpoint::restore(&workspace, bundle.as_deref(), patch.as_deref())
                .await?;

        use offload_workspace::checkpoint::BundleOutcome;
        let how = match (&outcome, patch.is_some()) {
            (BundleOutcome::Applied { .. }, true) => "rebuilt from bundle and patch",
            (BundleOutcome::Applied { .. }, false) => "rebuilt from bundle",
            (BundleOutcome::AlreadyHere, true) => {
                "the commits here already held this checkpoint; patch reapplied"
            }
            (BundleOutcome::AlreadyHere, false) => "the commits here already held this checkpoint",
            (BundleOutcome::Absent, true) => "re-checked out, patch reapplied",
            (BundleOutcome::Absent, false) => "re-checked out",
        };
        // Commits on a leg the fleet has moved past, named the way the superseded *checkout* is
        // named below and for the same reason. `reset --hard` does not delete them, but a hash
        // nobody was told is a hash nobody finds — and the reflog that holds them expires.
        let how = match &outcome {
            BundleOutcome::Applied {
                left_behind: Some(left),
            } => {
                // Durably, and here rather than only in the run's log: these commits exist
                // nowhere else in the fleet, and the sentence below is read by whoever is
                // following the run — which is somebody on the *holder*, not necessarily this
                // machine, and nobody at all once the run is deleted.
                self.note_rescued(
                    run_id,
                    offload_core::Rescue::Commits {
                        named: left.kept_as.is_some(),
                    },
                );
                let commit = &left.commit;
                match &left.kept_as {
                    Some(kept) => format!(
                        "{how}; this branch was at {commit}, which this checkpoint does not \
                         contain — those commits are kept at {kept}"
                    ),
                    // Worth saying rather than implying: this is the case where the reflog
                    // really is all there is, and it expires.
                    None => format!(
                        "{how}; this branch was at {commit}, which this checkpoint does not \
                         contain, and it could not be given a name — only the reflog has it"
                    ),
                }
            }
            _ => how.to_string(),
        };
        let how = how.as_str();
        // The rebuilt checkout holds exactly this checkpoint, which is what the next resume
        // compares itself against.
        self.workspaces.note_turn(run_id, checkpoint.turns);

        // A superseded checkout is named in the run's own log rather than only in `tracing`:
        // it may hold the only copy of a mid-turn edit, and a path nobody was told is a path
        // nobody finds. `Resumed.workspace` exists for exactly this — how it came back is
        // what matters when a resumed run turns out to be missing something.
        let how = match rescued {
            Some((at_turn, offload_workspace::Rescued::MovedAside(moved))) => format!(
                "{how}; the checkout here held turn {} and uncommitted work, so it is kept at {}",
                at_turn.map_or_else(|| "unknown".to_string(), |t| t.to_string()),
                moved.display()
            ),
            // Said as well as the one above, because a rescue and a removal are what the person
            // looking for a file needs told apart — and the removal is the *ordinary* case, so a
            // line that only ever appears for the rescue teaches nobody to expect either.
            Some((at_turn, offload_workspace::Rescued::Redundant)) => format!(
                "{how}; the checkout here held turn {} and nothing uncommitted, so it was \
                 removed — its commits are on the run branch",
                at_turn.map_or_else(|| "unknown".to_string(), |t| t.to_string()),
            ),
            None => how.to_string(),
        };
        Ok((
            workspace,
            Restored {
                how,
                adopted: false,
            },
        ))
    }

    /// Put the agent's transcript where it will be found for this worktree.
    ///
    /// Only when it is missing, or when the checkout beside it turned out to be from an
    /// earlier leg. A transcript already at the expected path belongs to this same session
    /// and — as long as the worktree it names was adopted — is at least as current as the
    /// checkpoint's copy, so overwriting it would roll the conversation back to the last
    /// capture for no reason.
    fn install_transcript(
        &self,
        workspace: &Workspace,
        session: &SessionId,
        checkpoint: &Checkpoint,
        restored: &Restored,
    ) -> Result<(), SubmitError> {
        // `expected_path`, not `find`. `find` falls back to scanning every project directory
        // for the session id, which is right when *reading* a transcript whose slug rule may
        // have changed — and wrong here: on a migration between two nodes sharing a home
        // directory it finds the *other* node's copy, under the other node's worktree path,
        // and skips the install. The agent then computes its own path from its own cwd, finds
        // nothing, and reports "no conversation found with session ID" — a resume that starts
        // a brand new conversation while claiming to continue one.
        //
        // And only when the worktree it belongs to was adopted. A transcript's path is
        // derived from the worktree path, so the file an earlier leg left here sits at
        // exactly the path this leg computes — and keeping it would resume the conversation
        // as this node last saw it rather than as it stands. One question decides both: if
        // the checkout here was not current, neither is the transcript beside it.
        if restored.adopted
            && transcript::expected_path(&self.home, &workspace.path, session.as_str()).is_file()
        {
            return Ok(());
        }
        let bytes = self.store.get_blob(checkpoint.transcript)?;
        let path = transcript::install(&self.home, &workspace.path, session.as_str(), &bytes)
            .map_err(|e| SubmitError::Agent(e.to_string()))?;
        tracing::info!(path = %path.display(), "transcript restored from checkpoint");
        Ok(())
    }

    fn record_event(&self, run_id: RunId, event: AgentEvent) {
        let kind = match event {
            AgentEvent::Started {
                model,
                agent_version,
                session_id,
                ..
            } => LogKind::AgentStarted {
                model,
                version: agent_version,
                session: session_id,
            },
            AgentEvent::Text { text } => LogKind::Text { text },
            AgentEvent::ToolUse { name, .. } => LogKind::ToolUse { name },
            AgentEvent::TurnBoundary { turn } => {
                self.update_stats(run_id, |stats| stats.turns = turn);
                LogKind::TurnBoundary { turn }
            }
            AgentEvent::RateLimit {
                kind,
                status,
                resets_at,
            } => {
                // The agent counts in unix seconds and everything here is milliseconds. The
                // log used to drop this field entirely, which was harmless while nothing
                // judged a rate limit against anything.
                let resets_at_unix_ms = resets_at.map(|secs| secs.saturating_mul(1_000));
                self.append(
                    run_id,
                    LogKind::RateLimit {
                        kind: kind.clone(),
                        status: status.clone(),
                        resets_at_unix_ms,
                    },
                );
                self.judge_rate_limit(run_id, &kind, &status, resets_at_unix_ms);
                return;
            }
            AgentEvent::Finished(outcome) => {
                let cost = outcome.cost_micro_usd.unwrap_or(0);
                let epoch = lock(&self.live).get(&run_id).map(|l| l.epoch);

                if let Some(epoch) = epoch {
                    // Turns are already cumulative — `accumulate_turns` offsets them on the
                    // way in — but cost and denials arrive per *process*, and a resumed agent
                    // is a new one. Added rather than assigned, or a run resumed at turn 20
                    // reports the price of its last leg as the price of the run.
                    self.update_stats(run_id, |stats| {
                        stats.turns = outcome.turns.max(stats.turns);
                        stats.denials = stats.denials.saturating_add(outcome.permission_denials);
                        stats.cost_micro_usd = stats.cost_micro_usd.saturating_add(cost);
                    });

                    // The transition is applied to the row rather than to a copy of it: this is
                    // the last thing that will ever be said about the run, and a checkpoint
                    // written a moment ago by the task that is winding this agent down is
                    // exactly what a whole-row write from an older copy would take with it.
                    // Nothing inside the closure may touch the store — it is holding its lock.
                    let stop_reason = outcome.stop_reason.clone();
                    let settled = self.store.update_run(run_id, |run| {
                        let result = if outcome.success {
                            run.complete(self.node_id, epoch, now())
                        } else {
                            let reason =
                                stop_reason.unwrap_or_else(|| "agent reported failure".to_string());
                            run.fail(self.node_id, epoch, reason, now())
                        };
                        (result, run.state.is_failed())
                    });
                    if let Ok(Some((result, is_failed))) = settled {
                        if is_failed {
                            // Before the terminal event goes out, because that event is what
                            // makes every follower hang up.
                            self.note_failure(run_id);
                        }
                        if let Err(e) = result {
                            // A fencing rejection here would mean something reassigned this run
                            // underneath us, which cannot happen on one node — worth shouting
                            // about rather than swallowing.
                            tracing::error!(run_id = %run_id, error = %e, "terminal transition refused");
                        }
                    }
                }
                LogKind::Finished {
                    success: outcome.success,
                    turns: outcome.turns,
                    denials: outcome.permission_denials,
                    cost_micro_usd: cost,
                    work: offload_core::WorkKind::Agent,
                    // What a continuation is handed (ADR-0064 §3). Moved, not read: Offload
                    // does nothing with it but keep it.
                    result: offload_core::event::closing_message(outcome.result.clone()),
                }
            }
            AgentEvent::ToolResult { .. } | AgentEvent::Unrecognized { .. } => return,
        };
        self.append(run_id, kind);
    }

    /// Is this rate limit longer than the run can afford? (ADR-0013)
    ///
    /// The whole of what "wait for the reset if it fits, surface it if it does not" can honestly
    /// mean today. There is nothing to *do* about a spent account: the agent is what waits,
    /// per-account limits are shared by every node using the same login (open question #5), and
    /// moving the run buys nothing because it takes its account with it. What was missing was
    /// telling anybody — the event was recorded and had no consequence, so a run whose reset
    /// lands two hours the wrong side of its deadline looked exactly like one that was fine.
    ///
    /// Judged the instant the agent reports it, not when the deadline arrives: at 07:00 a run
    /// due at 08:00 is in no trouble at all, and a limit that lifts at 10:00 has already
    /// decided the matter. Waiting to find out would waste the hour somebody could have used.
    fn judge_rate_limit(
        &self,
        run_id: RunId,
        kind: &str,
        status: &str,
        resets_at_unix_ms: Option<u64>,
    ) {
        // The agent emits one of these per turn either way, and an informational one is not a
        // delay. Without this check, every overdue run's quota report is an alarm.
        if !offload_core::rate_limit_blocks(status) {
            return;
        }
        let Some(run) = self.run(run_id) else {
            return;
        };
        // Remembered, so that the *next* run is not started into the same wall (ADR-0029). Only
        // where the agent said when it lifts: a block with no stated reset is a fact with no
        // expiry, and this codebase has the lesson twice already — a flag that never clears is
        // machinery for a state nothing ends, and the run that hit it is affected anyway.
        if let Some(resets_at) = resets_at_unix_ms {
            self.note_account_limit(kind, Millis(resets_at));
        }
        // A limit that did not say when it lifts is a wait with no known end, so it is judged
        // against now — which announces only a run that is already past due.
        let lifts_at = resets_at_unix_ms.map_or_else(now, Millis);
        if let Prospect::Missed { by } = run.prospect_at(lifts_at) {
            self.note_overdue(
                &run,
                by,
                Waiting::RateLimit {
                    kind: kind.to_string(),
                    resets_at_unix_ms,
                },
            );
        }
    }

    /// Note that one of the account's limits is blocking, and when it lifts.
    ///
    /// Latest wins per kind, which is not "most recent wins": the agent restates the same limit
    /// every turn, so an equal or earlier reset for a kind we already hold is the same fact told
    /// again and moving the value backwards would let a run start into a limit still in force.
    fn note_account_limit(&self, kind: &str, lifts_at: Millis) {
        let mut limits = lock(&self.limits);
        let entry = limits.entry(kind.to_string()).or_insert(lifts_at);
        if lifts_at > *entry {
            *entry = lifts_at;
        }
        tracing::info!(
            kind,
            lifts_at_unix_ms = lifts_at.0,
            "the account is rate-limited; holding new runs until it lifts"
        );
    }

    /// When the account's rate limit lifts, if any of them is still holding (ADR-0029).
    ///
    /// The latest of them, because they are independent ceilings and a run has to clear all of
    /// them. Expired entries are dropped on the way past — the map is the whole state and an
    /// entry that has lifted is not a fact any more, so there is nothing else to garbage-collect
    /// and no tick that has to remember to.
    ///
    /// Resolved here rather than in `offload-core`, which has no clock: what leaves this function
    /// is a decision, and every reader of it is spared asking whether the value is stale.
    #[must_use]
    pub fn account_limited_until(&self) -> Option<Millis> {
        let now = now();
        let mut limits = lock(&self.limits);
        limits.retain(|_, lifts_at| *lifts_at > now);
        limits.values().copied().max()
    }

    /// Drop the process-scoped handle for a run that is no longer ours to act on.
    ///
    /// The **handle**, and deliberately not the entry or the stream: `drive` calls this before it
    /// writes the run's terminal state, so that a `cancel` or a `resume` arriving in between finds
    /// no agent — and the last events of the run are appended *after* it. What ends the stream is
    /// [`Self::close_stream`], one step further on.
    fn release(&self, run_id: RunId) {
        if let Some(entry) = lock(&self.live).get_mut(&run_id) {
            entry.cancel = None;
        }
    }

    /// Forget the legs of runs whose rows have gone.
    ///
    /// The other half of [`Self::close_stream`], and the reason that one keeps the entry: what it
    /// keeps is a fact *about a run*, so it has no business outliving the run's own row. Nothing
    /// removed a `live` entry at all before this pair, and ADR-0020 is what turns "a few bytes
    /// per run for as long as the daemon lives" into a number — a rule firing every three seconds
    /// is 1,200 runs an hour, and their records are pruned while their legs were not.
    ///
    /// Safe by construction rather than by care: `describes` and `holds` both start from
    /// `self.run(run_id)`, which is `None` for a row that is gone, so there is no answer this can
    /// change. What it removes is the memory of an answer nobody can ask for.
    pub fn forget_legs(&self, runs: &[RunId]) {
        if runs.is_empty() {
            return;
        }
        let mut live = lock(&self.live);
        for run in runs {
            live.remove(run);
        }
    }

    /// This leg will append nothing more: let the run's event channel go.
    ///
    /// The entry stays, because [`Self::describes`] needs this leg's epoch to tell a run that
    /// ended *here* from one that ended somewhere else, and there is nowhere else that fact
    /// lives. The channel is the part worth reclaiming: `broadcast::channel` allocates its ring
    /// when it is created, so an entry costs about **37 KB** whether anybody followed the run or
    /// not — measured, 200 finished runs against a resident-set delta, with the store kept out of
    /// it. Nothing removed a `live` entry before this, so that was 37 KB per run for as long as
    /// the daemon lived, and ADR-0020's triggers make runs cheap: a rule firing every three
    /// seconds is 1,200 an hour.
    ///
    /// Called from `launch` rather than from `drive`, because it has to be after *every* way the
    /// leg can end — including `fail`, which appends the run's last line and runs in the arm
    /// `drive` returned an error to.
    ///
    /// A follower still attached drains what is buffered and then sees the channel close, which
    /// for `offload logs -f` is the run ending — the thing it was waiting for.
    fn close_stream(&self, run_id: RunId) {
        if let Some(entry) = lock(&self.live).get_mut(&run_id) {
            entry.live = None;
        }
    }

    /// Write the cancel down, if it actually applied.
    ///
    /// `Run::cancel` is unfenced but it still refuses a terminal run, and the window where that
    /// happens is the one somebody is most likely to be typing in: the agent reports its result,
    /// the run goes `Completed`, and the stream has not closed yet — so the next turn of the
    /// select loop, which is *biased* towards the cancel channel, picks up a cancel that arrived
    /// a moment ago. The transition is refused, correctly, and the log line used to be written
    /// anyway. `LogKind::Cancelled` is terminal to everything that reads it, so a run that had
    /// just succeeded had "cancelled" as the last word in its own log while the record said
    /// `completed` — a log kind contradicting the state it is named after, which is the mistake
    /// `CaptureFailed` exists to avoid.
    ///
    /// Nothing is written on the refusal because the line that belongs there is already there:
    /// whatever moved the run to its terminal state logged it on the way past.
    fn finish_cancelled(&self, run_id: RunId, by: &str) {
        let applied = matches!(
            self.store.update_run(run_id, |run| run.cancel(now())),
            Ok(Some(Ok(())))
        );
        if let Some(entry) = lock(&self.live).get_mut(&run_id) {
            entry.cancel = None;
        }
        if applied {
            self.append(run_id, LogKind::Cancelled { by: by.to_string() });
        } else {
            tracing::info!(
                run_id = %run_id,
                %by,
                "the cancel lost the race with the run finishing; leaving its own record alone"
            );
            self.note_refused(run_id, offload_core::Attempt::Cancel);
        }
    }

    /// Give up on a run **this node holds**, and say so in its log.
    ///
    /// `abandon`, not `fail`: the agent may never have started, so there is no holder to fence
    /// against — which is right for the failure this exists for and is precisely why the guard
    /// has to be here. An unfenced terminal write is the most dangerous thing in this file: it
    /// beats a live record everywhere, and `note_failure` then makes the run a candidate for
    /// auto-resume, so a run somebody else is working on can be declared dead *and* started a
    /// second time.
    ///
    /// The check used to live in the callers, and this session found four separate paths that
    /// wrote about a run they had lost — each guard correct, each caller a different oversight.
    /// One writer asking one question is the version that cannot be forgotten by the next caller
    /// somebody adds. `Supervisor::recover` keeps its own copy of the check because it runs before
    /// any of this exists, and says so.
    fn fail(&self, run_id: RunId, reason: String) {
        if !self.holds(run_id) {
            tracing::warn!(
                run_id = %run_id,
                %reason,
                "not failing a run this node does not hold"
            );
            self.note_refused(run_id, offload_core::Attempt::Fail);
            return;
        }
        // `Ok(Some(Ok(())))`, not `Ok(Some(_))`: `abandon` still refuses a run that has already
        // ended, and the log line must not claim a failure the record does not have. That is
        // `finish_cancelled`'s lesson, which this had in exactly the same shape.
        let applied = matches!(
            self.store
                .update_run(run_id, |run| run.abandon(reason.clone(), now())),
            Ok(Some(Ok(())))
        );
        if applied {
            self.note_failure(run_id);
        }
        if let Some(entry) = lock(&self.live).get_mut(&run_id) {
            entry.cancel = None;
        }
        if applied {
            self.append(run_id, LogKind::Failed { reason });
        } else {
            tracing::info!(run_id = %run_id, %reason, "the run had already ended; not failing it");
        }
    }

    /// An agent that stopped without saying how it went.
    ///
    /// `pump` ends when the event stream closes, which normally happens *after* the agent's
    /// `Result` event has already moved the run to `Completed` or `Failed`. When the process
    /// dies instead — crashed, killed, out of memory, or a binary that is not an agent at all —
    /// there is no result, and until this existed the run stayed `Running` for ever: its holder
    /// is alive and renewing the lease, so nothing orphans it, the failure detector has nothing
    /// to detect, and the only symptom is a row in `ps` that never changes again. Found by
    /// pointing `agent.binary` at `/bin/false`, which is the cheapest possible dead agent.
    ///
    /// Narrow on purpose: only a run this node is still *holding under a lease* is failed here.
    /// A completed one has said its piece, and a checkpointed one is `Pending` and belongs to
    /// whoever picks it up next.
    fn fail_if_unfinished(&self, run_id: RunId) {
        // `still_ours` has to mean *ours*, which is a question about the holder and the epoch and
        // not about the state. It used to test only that the run was `Running` or
        // `Checkpointing` — and a run reassigned to another machine is `Running` too, so a leg
        // whose agent crashed at the moment it lost the run wrote `Failed` over the new holder's
        // record: unfenced, at their epoch, terminal, and therefore winning everywhere while
        // their agent worked on. Reachable by ordinary luck rather than by a race anybody has to
        // arrange — `record_run` asks the agent here to stop, and an agent that dies of its own
        // accord first makes the pump report `Finished` rather than `Halted`.
        //
        // The epoch as well as the holder, for `record_run`'s reason: a grant at our own epoch
        // naming somebody else is a run granted twice, and this leg is not the one entitled to
        // record its ending either way.
        let live_epoch = lock(&self.live).get(&run_id).map(|l| l.epoch);
        let still_ours = self.run(run_id).is_some_and(|run| {
            run.holder() == Some(self.node_id)
                && live_epoch.is_none_or(|epoch| epoch == run.epoch)
                && matches!(
                    run.state,
                    RunState::Running { .. } | RunState::Checkpointing { .. }
                )
        });
        if still_ours {
            self.fail(
                run_id,
                "the agent stopped without reporting a result".to_string(),
            );
        }
    }

    /// Change this node's own numbers for a run, and stamp them — twice.
    ///
    /// Every local write goes through here so neither stamp can be forgotten. The **clock** is
    /// what tells a peer that two records with the same turns and the same cost are not the same
    /// record (`offload_core::progress`), and a write that skipped it would be silently unable to
    /// correct anything it had already said. The **leg** is what tells a peer that two records
    /// with different turn counts are not the same *run's* position — the newer one may belong to
    /// a leg that lost, in which case its larger number is not progress at all.
    fn update_stats(&self, run_id: RunId, change: impl FnOnce(&mut offload_core::RunProgress)) {
        // The leg is worked out before the store's lock is taken, because [`Self::writing_leg`]
        // reads the run registry and would otherwise be asking for a lock it is already holding.
        let leg = self.writing_leg(run_id);
        let _ = self.store.update_stats(run_id, |stats| {
            // **A row that speaks in another leg's name is not this node's to write into.** With
            // no leg of our own there is nothing to sign a correction with, and writing anyway
            // does not merely record a local fact: `at` is part of `RunProgress::position`, so the
            // touch alone makes this row beat the peer's own copy on the tiebreak and carries our
            // sentence back to them under their name. Measured: `offload rm` on a node holding a
            // leftover checkout told the peer that the peer's checkout was gone, while it sat
            // there untouched — session seventeen's bug, reached through the half of the door its
            // fix does not cover, because `remove` really does return `Removed` here.
            //
            // The two situations [`Self::writing_leg`] answers `None` for are opposite, and this
            // is what tells them apart: after a restart `live` is empty for runs that ended
            // *elsewhere* as well as for runs that ended here, and only in the second case is the
            // stamp already on the row our own. An unstamped row is nobody's and stays writable.
            if leg.is_none() && stats.by.is_some_and(|by| by != self.node_id) {
                return;
            }
            let cost_before = stats.cost_micro_usd;
            change(stats);
            stats.at = now();
            // Stamped by the writer, for the reason `fail` refuses a run this node does not hold:
            // five call sites are five things to remember and the sixth will not. It is what lets
            // `RunProgress::absorb` *rank* this position against another leg's rather than merely
            // find it larger — see [`Self::writing_leg`] for which leg it claims to be.
            if let Some((by, epoch)) = leg {
                stats.by = Some(by);
                stats.epoch = epoch;
                // This leg's own spend (ADR-0067): its transcript as captured, and whatever cost
                // this write added. Only the writing leg's entry, and only by its own writer.
                let added = stats.cost_micro_usd.saturating_sub(cost_before);
                let tokens = stats.tokens;
                // Updated here, **never created here**. Only the leg's start knows its base (the
                // restored transcript's count), and an entry created by whichever write came first
                // takes the row's tokens at that moment instead — measured on a drain: the new
                // leg's "workspace ready" write created it at base 110 (the old leg's turn-1
                // figure, still in the row) when the checkpoint it resumed from held 220, and an
                // ordinary migration then reported `+110 tokens spent by a leg that lost`. A
                // missing entry can only under-count; a guessed base raises a false alarm on every
                // migration. See `begin_leg`.
                if let Some(entry) = stats
                    .legs
                    .iter_mut()
                    .find(|l| l.by == by && l.epoch == epoch)
                {
                    entry.tokens = entry.tokens.max(tokens);
                    entry.cost_micro_usd = entry.cost_micro_usd.saturating_add(added);
                }
            }
        });
    }

    /// Whose numbers these are: this node, and the epoch it is writing them under.
    ///
    /// The live leg's epoch first, which is the answer while an agent is running and is a memory
    /// read rather than a store one — the turn boundary is the daemon's one hot path, and it is
    /// why stamping is affordable where the `holds` check it stands in for was not.
    ///
    /// Then the record's, for the two writes that happen with no agent: "waiting for a slot" and
    /// "preparing" before one starts, and "removed" long after one ended. [`Self::describes`] is
    /// the question — may this leg say what its worktree holds — asked here for exactly the
    /// reason it exists one function up.
    ///
    /// `None` stamps nothing and leaves whatever the row already carried. That is deliberate for
    /// the case it covers, a note about a run that ended here and whose `live` entry is long
    /// gone: the stamp already on the row is this leg's own, and inventing a fresh one at the
    /// record's *current* epoch would have our note claim to be a later leg's.
    ///
    /// It covers a **second** situation that looks identical here and is its opposite, which is
    /// why the caller does not treat `None` as permission on its own: after a restart `live` is
    /// empty for runs that ended *elsewhere* too, and the stamp sitting on those rows is the
    /// peer's. [`Self::update_stats`] asks whose the stamp is before it writes.
    fn writing_leg(&self, run_id: RunId) -> Option<(NodeId, Epoch)> {
        if let Some(epoch) = lock(&self.live).get(&run_id).map(|l| l.epoch) {
            return Some((self.node_id, epoch));
        }
        if self.describes(run_id) {
            return self.run(run_id).map(|run| (self.node_id, run.epoch));
        }
        None
    }

    /// Is this run this node's to *conclude* — to write a terminal state for?
    ///
    /// Holder *and* epoch, for `record_run`'s reason: a record naming somebody else at our own
    /// epoch is a run granted twice, and neither leg is entitled to end it.
    ///
    /// False for a run that has already finished, because a finished run has no holder — the
    /// lease goes with the terminal transition — and that is the right answer: it is over, and
    /// there is nothing left to conclude.
    fn holds(&self, run_id: RunId) -> bool {
        let live_epoch = lock(&self.live).get(&run_id).map(|l| l.epoch);
        self.run(run_id).is_some_and(|run| {
            run.holder() == Some(self.node_id) && live_epoch.is_none_or(|epoch| epoch == run.epoch)
        })
    }

    /// Is this leg still the run's current one — may it say what its worktree holds?
    ///
    /// A different question from [`Self::holds`], and separating them is not pedantry. A run that
    /// **finished here** is exactly the case somebody wants a final worktree summary for — "3
    /// modified" is what they read afterwards — and it has no holder to check. A run that
    /// **moved** must not be described by the machine it left. The epoch tells those apart where
    /// the holder cannot: the terminal transitions are fenced, so a run that ended here ended at
    /// this leg's epoch, while one that moved is at a higher one.
    ///
    /// Collapsing the two cost a false audit row before it cost anything else — a completed run
    /// reported as a refused write, which is a fence somebody would have gone looking for.
    fn describes(&self, run_id: RunId) -> bool {
        let live_epoch = lock(&self.live).get(&run_id).map(|l| l.epoch);
        self.run(run_id).is_some_and(|run| match run.holder() {
            // Somebody holds it, so the only question is whether that is us — and at our own
            // epoch, because an equal-epoch grant naming another node is a run granted twice and
            // neither leg may speak for it.
            Some(holder) => {
                holder == self.node_id && live_epoch.is_none_or(|epoch| epoch == run.epoch)
            }
            // Nobody holds it, which for a run that reached this point means it ended. Ended
            // *here* if it ended at this leg's epoch — the terminal transitions are fenced, so no
            // other node could have written one under our number.
            None => live_epoch.is_some_and(|epoch| epoch == run.epoch),
        })
    }

    /// Write down that this node was refused a write about a run (`offload_core::audit`).
    ///
    /// Called from the guards rather than from inside them, so each site names what it was
    /// attempting: the useful row says "tried to start it" or "tried to fail it", and a row that
    /// said only "fenced" would leave the reader to guess which of the four it was.
    fn note_refused(&self, run_id: RunId, attempt: offload_core::Attempt) {
        let held = lock(&self.live)
            .get(&run_id)
            .map_or(offload_core::run::Epoch(0), |l| l.epoch);
        self.store.append_audit(&offload_core::AuditEntry {
            at_unix_ms: now().0,
            kind: offload_core::AuditEvent::Refused {
                run: run_id,
                attempt,
                held,
            },
        });
    }

    /// Write down that this node, arbitrating, gave a run to somebody.
    pub fn note_granted(&self, run_id: RunId, to: NodeId, epoch: Epoch) {
        self.store.append_audit(&offload_core::AuditEntry {
            at_unix_ms: now().0,
            kind: offload_core::AuditEvent::Granted {
                run: run_id,
                to,
                epoch,
            },
        });
    }

    /// And that it took one, under the epoch it was granted at.
    fn note_accepted(&self, run_id: RunId, epoch: Epoch) {
        self.store.append_audit(&offload_core::AuditEntry {
            at_unix_ms: now().0,
            kind: offload_core::AuditEvent::Accepted { run: run_id, epoch },
        });
    }

    /// And that the fleet took one away, with the turn this leg had reached.
    ///
    /// The turn comes from this node's own numbers rather than from the record, because it is this
    /// leg's position that is about to stop being the run's — the record already carries the
    /// survivor's. Read after `save_run`, which is safe for the reason the two are separate calls
    /// at all: `save_run` writes the domain object and deliberately does not touch the stats
    /// columns beside it.
    fn note_superseded(
        &self,
        run_id: RunId,
        held: Epoch,
        to: NodeId,
        epoch: Epoch,
        finished: bool,
    ) {
        let turns = self.store.load_stats(run_id).unwrap_or_default().turns;
        self.store.append_audit(&offload_core::AuditEntry {
            at_unix_ms: now().0,
            kind: offload_core::AuditEvent::Superseded {
                run: run_id,
                held,
                to,
                epoch,
                turns,
                finished,
            },
        });
    }

    /// And that it took a checkout of its own off the disk (ADR-0023).
    ///
    /// The fifth `note_*`, and the first that is not about the fleet's opinion of a run at all.
    /// Its sibling [`Self::note_workspace`] is the shape this could not use: `RunProgress` is a
    /// fleet-agreed record with one writing leg, and the sweep runs on the leg that has *lost*
    /// the run — so `update_stats` would drop the write (correctly, since a leg that lost may not
    /// describe a worktree, session seventeen), and the removal would go on being invisible. The
    /// same fact in the per-node log has no such problem, because no other node has a copy of it
    /// to disagree with.
    ///
    /// No epoch on the row, and that is deliberate rather than an omission. Every other entry
    /// here is about authority over a *run* and an epoch is what authority is measured in; this
    /// one is about a *directory*, and this node's authority over its own disk is not fenced by
    /// anything. Putting a number there that decides nothing is how a reader learns to distrust
    /// the ones that do.
    fn note_reclaimed(&self, run_id: RunId, why: offload_core::Reclamation) {
        self.store.append_audit(&offload_core::AuditEntry {
            at_unix_ms: now().0,
            kind: offload_core::AuditEvent::Reclaimed { run: run_id, why },
        });
    }

    /// And that it kept something of its own that nothing will ever come back for.
    ///
    /// The sixth `note_*`, and [`Self::note_reclaimed`]'s counterpart in every respect: a fact
    /// about one disk, no epoch on the row because a directory is not fenced by anything, and a
    /// closed set for the *which* rather than a sentence. Both of the things it records were
    /// previously a `tracing::warn!` and a line in the **run's** log — which is served by the
    /// run's holder, and the leg that rescues is by construction the leg that lost it.
    fn note_rescued(&self, run_id: RunId, what: offload_core::Rescue) {
        self.store.append_audit(&offload_core::AuditEntry {
            at_unix_ms: now().0,
            kind: offload_core::AuditEvent::Rescued { run: run_id, what },
        });
    }

    /// Say what is happening to the run's worktree, leaving every other number alone.
    ///
    /// The distinction that matters: the workspace note is this leg's business, and the turns
    /// and the bill are the run's.
    fn note_workspace(&self, run_id: RunId, note: &str) {
        self.update_stats(run_id, |stats| stats.workspace = note.to_string());
    }

    /// Stamp the leg without claiming anything about the run's progress.
    ///
    /// [`Self::update_stats`] stamps `by`, `epoch` and `at` on every write, and every other
    /// caller has a number to change as well. A task has none — no turns, no bill, no worktree
    /// summary (ADR-0019 §2) — and still needs the stamp, because the stamp is what says *which
    /// machine holds this run's log* ([`crate::server::log_source`], via [`Self::progress_leg`]).
    /// So the closure is empty on purpose: the write is the signature, not the numbers.
    ///
    /// It is a named method rather than an inline `|_| {}` for the reason `note_workspace` is
    /// one: a reader who finds `update_stats(id, |_| {})` at a call site cannot tell a deliberate
    /// stamp from a leftover, and the next person to tidy it away would take the log with it.
    fn note_log_leg(&self, run_id: RunId) {
        self.update_stats(run_id, |_| {});
    }

    async fn refresh_workspace_summary(&self, run_id: RunId, workspace: &Workspace) {
        let summary = self
            .workspaces
            .status(workspace)
            .await
            .map_or_else(|_| "unknown".to_string(), |s| s.summary());
        self.update_stats(run_id, |stats| stats.workspace = summary);
    }

    /// Record an event durably, then tell anyone following.
    ///
    /// Store first: a follower that sees an event the store does not have would show
    /// output that vanishes on reconnect.
    /// The hook this run's agent asks through, or `None` if this node cannot offer one.
    ///
    /// The program is **this daemon's own binary**, which is the whole reason there is no
    /// configuration here: `current_exe` is exact, needs no discovery, and cannot be pointed at a
    /// different version of the protocol. A node that cannot say where its own binary is says so
    /// and runs the way it always has, rather than spawning an agent whose questions go nowhere.
    fn ask_hook(&self, run: &Run) -> Option<offload_agent::AskHook> {
        let command = match std::env::current_exe() {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(
                    run_id = %run.id,
                    error = %e,
                    "this run asked to be able to ask a person, and this node cannot find its \
                     own binary to ask through; it will be denied as usual instead"
                );
                return None;
            }
        };
        // The hook's own timeout is a backstop, not the decision: the daemon answers on its own
        // patience and the margin is only so a killed hook cannot outlive the answer. A deadline
        // edited after the agent started leaves this stale, which costs nothing — whichever of
        // the two is shorter decides, and both mean "nobody answered".
        let patience = run.approval_patience(now(), DEFAULT_ASK_PATIENCE, MIN_ASK_PATIENCE);
        Some(offload_agent::AskHook {
            command,
            args: vec![ASK_HOOK_ARG.to_string()],
            timeout: std::time::Duration::from_millis(patience.0) + ASK_HOOK_MARGIN,
            tools: ask_tools(run.spec.agent()?.permission_mode)
                .iter()
                .map(|t| (*t).to_string())
                .collect(),
        })
    }

    /// An agent is blocked and wants to know whether it may do something (ADR-0017).
    ///
    /// Returns the wait, or the reason there is nothing to wait for. Four refusals, and each one
    /// is a case where asking a person would be wrong rather than merely unnecessary:
    ///
    /// * **A run this node does not hold at this epoch.** The hook belongs to one leg of a run;
    ///   if the run has moved, its old agent's questions are not current and its side effects are
    ///   already fenced off. Same rule as every other effect path.
    /// * **A run that never asked to be able to ask.** Defensive: the hook is only configured for
    ///   a run whose spec says so, and a question arriving anyway is a bug rather than a person's
    ///   decision to make.
    /// * **A call an existing grant already covers.** The operator has decided about this; asking
    ///   again would teach them to stop reading. A match here means *do not ask* and never
    ///   *allow* — the agent still applies its own rules — so a grant this node reads more
    ///   generously than the agent does costs a question, not a permission.
    /// * **Nothing that could reach a person.** No route in the run's audience and nobody
    ///   watching the stream: the question would stall the run for minutes and then be answered
    ///   by the clock. Better to decline instantly and let the agent do what it does today.
    pub fn ask(
        &self,
        run_id: RunId,
        epoch: Epoch,
        tool_use_id: &str,
        tool: &str,
        detail: &str,
        reachable: bool,
    ) -> Result<AskWait, NoAsk> {
        let run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| NoAsk::Unknown(run_id.to_string()))?;
        if run.holder() != Some(self.node_id) || run.epoch != epoch {
            return Err(NoAsk::NotOurs { epoch: run.epoch.0 });
        }
        // A task has no ask policy at all, and `NotAsking` is the truthful answer rather than
        // a stand-in: nothing about it can stop and ask, because there is no tool call to gate.
        let Some(work) = run.spec.agent() else {
            return Err(NoAsk::NotAsking);
        };
        if !work.ask.enabled() {
            return Err(NoAsk::NotAsking);
        }
        if work.allow.covers(tool, detail) {
            return Err(NoAsk::AlreadyGranted);
        }
        // After the grant check and before the reachability one, which is the order the reasons
        // want: a call somebody has already decided about should not spend a question, and a
        // budget that is gone is a fact about this run rather than about the fleet's routes.
        let asked = self.store.load_stats(run_id).map(|s| s.asks).unwrap_or(0);
        if !work.ask.may_ask(asked) {
            self.note_budget_spent(run_id, asked);
            return Err(NoAsk::BudgetSpent { asked });
        }
        if !reachable && self.watchers(run_id).unwrap_or(0) == 0 {
            return Err(NoAsk::NobodyToAsk);
        }

        let now = now();
        let patience = run.approval_patience(now, DEFAULT_ASK_PATIENCE, MIN_ASK_PATIENCE);
        let answer = self.asks.register(
            crate::asks::Question {
                run: run_id,
                epoch,
                tool_use_id: tool_use_id.to_string(),
                tool: tool.to_string(),
                detail: detail.to_string(),
            },
            now,
            now.saturating_add(patience),
        );
        // Spent when the question is *put*, not when the hook fires: a call an existing grant
        // covers, or one nobody could have been asked about, has interrupted nobody and must not
        // count against what the operator agreed to answer.
        self.update_stats(run_id, |stats| stats.asks = stats.asks.saturating_add(1));
        // In the log, so it leaves the node: the delivery plane fans this out to whichever device
        // can reach a person (ADR-0010), and a question only this daemon knows about is a run
        // that quietly did less than it was asked.
        self.append(
            run_id,
            LogKind::Asked {
                tool: tool.to_string(),
                detail: detail.to_string(),
                tool_use_id: tool_use_id.to_string(),
                // Was `tracing`-only. The plane needs it: a question with no clock on it is the
                // one promise `Notice::NeedsDecision`'s doc comment says it cannot keep.
                within_ms: patience.0,
            },
        );
        tracing::info!(
            run_id = %run_id,
            %tool,
            %detail,
            patience = %patience,
            "a run is waiting for permission"
        );
        Ok(AskWait { answer, patience })
    }

    /// Say once that this run will not be asking anything else.
    ///
    /// Latched on the log rather than in memory, unlike the overdue notice: this one is decided by
    /// a counter that only goes up and travels with the run, so "have I already said it" is a
    /// question the log can answer on any node the run has been on — and a run migrating mid-way
    /// should not announce the same spent budget again on its new machine.
    fn note_budget_spent(&self, run_id: RunId, asked: u32) {
        let said = crate::api::logged_now(LogKind::AskBudgetSpent { asked });
        if self
            .store
            .has_event_kind(run_id, said.kind_name())
            .unwrap_or(false)
        {
            return;
        }
        tracing::info!(
            run_id = %run_id,
            asked,
            "this run has spent its ask budget; further calls are decided by the agent's own rules"
        );
        self.append(run_id, said.kind);
    }

    /// Record how a question ended and stop waiting for it.
    ///
    /// Called by the waiter in every case — answered, timed out, or the hook went away — because
    /// a registry that only forgot answered questions would keep offering an operator a question
    /// whose agent is long gone.
    pub fn asked_and_done(
        &self,
        run_id: RunId,
        tool_use_id: &str,
        answer: offload_core::Answer,
        by: &str,
        tool: &str,
    ) {
        self.asks.forget(run_id, tool_use_id);
        self.append(
            run_id,
            LogKind::Answered {
                tool_use_id: tool_use_id.to_string(),
                answer,
                by: by.to_string(),
                // Carried so the notification for the *undecided* case can name what was left to
                // the agent — a projection sees one event, and `tool_use_id` means nothing to a
                // person holding a phone.
                tool: tool.to_string(),
            },
        );
    }

    /// Answer a question, from an operator.
    pub fn answer(
        &self,
        run_id: RunId,
        which: Option<&str>,
        allow: bool,
        by: &str,
    ) -> Result<crate::asks::Answered, crate::asks::AnswerError> {
        self.asks.answer(
            run_id,
            which,
            crate::asks::Verdict {
                allow,
                by: by.to_string(),
            },
        )
    }

    /// What is waiting for a person on this node.
    #[must_use]
    pub fn asks(&self) -> Vec<offload_core::PendingAsk> {
        self.asks.waiting(now())
    }

    fn append(&self, run_id: RunId, kind: LogKind) {
        let event = crate::api::logged_now(kind);
        if let Err(e) = self
            .store
            .append_event(run_id, event.kind_name(), event.at_unix_ms, &event)
        {
            tracing::error!(run_id = %run_id, error = %e, "could not persist run event");
        }
        if let Some(live) = lock(&self.live).get(&run_id).and_then(|e| e.live.as_ref()) {
            // Failure means nobody is following, which is the normal case.
            let _ = live.send(event);
        }
    }

    /// `name_of` names a node for the `held` line; the supervisor has no view of its own.
    #[must_use]
    pub fn list(&self, name_of: &dyn Fn(NodeId) -> String) -> Vec<RunSummary> {
        let now = now();
        self.store
            .list_with_stats(true)
            .unwrap_or_default()
            .into_iter()
            .map(|(run, stats)| {
                let spent_cost = stats.spent().1;
                RunSummary {
                    id: run.id.to_string(),
                    state: run.state.name().to_string(),
                    kind: run.spec.work.kind(),
                    work: truncate(&run.spec.work.summary(), 60),
                    repo: run
                        .spec
                        .workspace()
                        .map(|w| w.repo.clone())
                        .unwrap_or_default(),
                    // Off the same `Option` as `repo`, and for the same reason: `run_branch` is a
                    // pure function of the run id, so it answers for a run that never checked
                    // anything out. A branch name is a claim that a branch exists.
                    branch: run
                        .spec
                        .workspace()
                        .map(|_| offload_workspace::run_branch(run.id))
                        .unwrap_or_default(),
                    turns: stats.turns,
                    tokens: stats.tokens,
                    lost_tokens: stats.lost_tokens(),
                    max_turns: run.spec.agent().and_then(|work| work.max_turns),
                    denials: stats.denials,
                    workspace: stats.workspace,
                    // Money spent, which after a fork is every leg's and not the larger one's
                    // (ADR-0067); along a chain the two are the same number.
                    cost_micro_usd: stats.cost_micro_usd.max(spent_cost),
                    started_at_unix: run.created_at.as_secs(),
                    error: failure_reason(&run),
                    // Paired by the reader with the node's standing answer, which travels beside
                    // the listing: the sentence `error` carries names `offload resume`, and this is
                    // half of whether that command would be taken here. `resumable_by_hand` rather
                    // than `resumable_state` because the footnote is advice to a *person*, and
                    // `offload resume` is not the command for a failed task on any node.
                    resumable: resumable_by_hand(&run),
                    origin: run.origin,
                    host: run.holder().or(stats.by).map(name_of),
                    model: run.spec.agent().and_then(|work| work.model.clone()),
                    // Only a *stated* deadline is worth a line: every run is technically due the
                    // instant it was submitted (ADR-0013), and printing "overdue by 4m" against
                    // work nobody is waiting for at a particular time would be noise that trains
                    // people to ignore the one run that means it.
                    due: run
                        .spec
                        .deadline
                        .filter(|_| !run.state.is_terminal())
                        .map(|_| run.slack(now).to_string()),
                    continues: run.spec.parent.as_ref().map(|p| p.run.to_string()),
                    held: (run.spec.holding(now) && !run.state.is_terminal()).then(|| {
                        let nodes: Vec<String> = run
                            .spec
                            .prefer
                            .named_nodes()
                            .into_iter()
                            .map(name_of)
                            .collect();
                        let left = run
                            .spec
                            .hold_until
                            .map_or(Millis(0), |until| until.saturating_sub(now));
                        if nodes.is_empty() {
                            format!("held for a node matching its preference, for another {left}")
                        } else {
                            format!("held for {}, for another {left}", nodes.join(" and "))
                        }
                    }),
                    // Only a run that can still lose its work has an answer to give, and that is
                    // `resumable_checkpoint` rather than `!is_terminal` — a `Failed` run is terminal
                    // and reopens, so it is precisely the run whose checkpoint the `here only` note
                    // is written for, and it was the one row that never showed it.
                    // And the question is whether the checkpoint survives *its holder* — asking
                    // about the local node reads "here only" on the very node holding the copy.
                    checkpoint_durable: run
                        .resumable_checkpoint()
                        .map(|c| c.is_durable(run.holder())),
                }
            })
            .collect()
    }

    /// The other continuations of `run`'s parent that this node has a record of, oldest first
    /// (ADR-0064 §6). Empty when `run` is not a continuation or is the only one.
    #[must_use]
    pub fn continuations_of_parent_of(&self, run: RunId) -> Vec<String> {
        let Some(parent) = self
            .store
            .load_run(run)
            .ok()
            .flatten()
            .and_then(|r| r.spec.parent.map(|p| p.run))
        else {
            return Vec::new();
        };
        let mut others: Vec<Run> = self
            .store
            .list_with_stats(true)
            .unwrap_or_default()
            .into_iter()
            .map(|(r, _)| r)
            .filter(|r| r.id != run && r.spec.parent.as_ref().is_some_and(|p| p.run == parent))
            .collect();
        others.sort_by_key(|r| r.created_at);
        others.into_iter().map(|r| r.id.to_string()).collect()
    }

    /// Resolve a full or abbreviated run id.
    pub fn resolve(&self, needle: &str) -> Result<RunId, SubmitError> {
        match self.store.resolve_run(needle) {
            Ok(id) => Ok(id),
            // Nothing about the store failed, so it must not be the thing named.
            Err(e @ offload_store::StoreError::NoRunGiven) => Err(SubmitError::Needle(e)),
            Err(e) => Err(e.into()),
        }
    }

    /// Everything logged so far, plus a subscription to what comes next.
    ///
    /// Both taken under one lock, because `append` also holds it — otherwise an event
    /// landing between the snapshot and the subscribe would be missed by a follower, and
    /// a missed turn boundary is a missed checkpoint opportunity.
    pub fn subscribe(
        &self,
        run_id: RunId,
    ) -> Result<(Vec<LogEvent>, Option<broadcast::Receiver<LogEvent>>), SubmitError> {
        // Subscribe *before* reading the backlog. The store is written before the
        // broadcast, so a subscriber attached first may see an event twice but can never
        // miss one — and a missed turn boundary is a missed checkpoint opportunity.
        let receiver = lock(&self.live)
            .get(&run_id)
            .and_then(|l| l.live.as_ref())
            .map(broadcast::Sender::subscribe);
        let backlog: Vec<LogEvent> = self
            .store
            .events_since(run_id, 0)?
            .into_iter()
            .map(|(_, event)| event)
            .collect();
        if backlog.is_empty() && receiver.is_none() {
            return Err(SubmitError::NoSuchRun(run_id.to_string()));
        }
        Ok((backlog, receiver))
    }

    #[must_use]
    pub fn is_terminal(&self, run_id: RunId) -> bool {
        self.store
            .load_run(run_id)
            .ok()
            .flatten()
            .is_none_or(|run| run.state.is_terminal())
    }

    /// Stop a run that is on **this** machine, and say what was stopped.
    ///
    /// The whole command rather than half of it. This used to be "signal the agent, if there is
    /// one", which answers a question about the process table — and the process table cannot tell
    /// a run that is not here from a run that is here and has not started. Both came back as
    /// "not running": the first a false sentence about a run spending money on the desktop, the
    /// second a false sentence about a commitment this very node was holding and about to begin.
    ///
    /// Three outcomes, and the note is the difference between them, because a person who cancels
    /// a mid-turn agent has spent money and a person who cancels a commitment has not:
    ///
    /// * an agent running here is signalled, and `drive` writes the terminal state when it stops;
    /// * a run held here that never started is written terminal now, from its own record;
    /// * a run whose record names **another** node is refused, naming it — writing a terminal
    ///   state for somebody else's run would win the merge (a terminal state beats anything at
    ///   the same epoch) while the agent kept working, which is this command's original bug with
    ///   the fleet convinced instead of one operator.
    ///
    /// `by` is the node the command was typed at, for the run's log.
    pub async fn cancel_run(&self, run_id: RunId, by: &str) -> Result<String, SubmitError> {
        let run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.short()))?;
        let refuse = |reason: String| SubmitError::Refused {
            run: run_id.short(),
            action: "cancelled",
            reason,
        };
        if run.state.is_terminal() {
            // A race rather than a fault: it finished while the command was in flight, which is
            // most of a second when the command crossed a network.
            return Err(refuse(format!("it is already {}", run.state.name())));
        }
        if let Some(holder) = run.holder().filter(|holder| *holder != self.node_id) {
            return Err(refuse(format!(
                "it is on {}, and a cancel has to reach the machine the run is on",
                holder.short()
            )));
        }

        // An agent here: signalled rather than written, because `drive` has to take its process
        // down and it is the one that writes the terminal state afterwards (`finish_cancelled`).
        // Writing it here as well would be two writers on one row for no benefit.
        if self
            .halt_agent(run_id, Halt::Cancelled { by: by.to_string() })
            .await
        {
            // Named by what it actually is. "its agent was stopped" is a sentence about a
            // model, and half of what this node can now be asked to stop is a nominated
            // program with no agent anywhere near it (ADR-0019).
            let what = self.run(run_id).map_or("work", |run| match run.spec.work {
                Work::Agent(_) => "agent",
                Work::Task(_) => "task",
            });
            return Ok(format!("its {what} was stopped"));
        }

        // Held here and not started, so there is nothing to interrupt and the record is the
        // whole of it. `Running` reaches here too, on a daemon that restarted: the row says
        // running and no process backs it, which is exactly the state `fail_if_unfinished`
        // exists for — closing it is the honest answer rather than leaving a run nothing will
        // ever move again.
        let note = match run.state.name() {
            "pending" => "it was waiting to be placed, and is not any more",
            "assigned" => "it had not started, so nothing was interrupted",
            _ => "no agent here was running it, so only its record was closed",
        };
        let never_started = run.state.name() == "assigned";
        self.store
            .update_run(run_id, |run| run.cancel(now()))?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.short()))?
            .map_err(|e| refuse(e.to_string()))?;
        // A run held and never started published its *refusal* into the worktree column, which
        // is the only thing there is to say about a run with no worktree. That sentence is
        // about a moment, and the moment has passed: left alone it gossips on as the fleet's
        // answer, so `offload ps` shows `cancelled` beside `waiting for a slot` for ever.
        // Only for the branch that never started — a `Running` row reaching here after a
        // restart has a checkout, and whatever the last leg said about it is still true.
        if never_started {
            self.note_workspace(run_id, NEVER_STARTED);
        }
        self.append(run_id, LogKind::Cancelled { by: by.to_string() });
        Ok(note.to_string())
    }

    /// Ask the agent running here to stop, and say why. `false` when there is none.
    ///
    /// Deliberately says nothing about whether the run *exists*: that is a question for the
    /// store, and answering it from the process table is what made `offload cancel` lie.
    async fn halt_agent(&self, run_id: RunId, why: Halt) -> bool {
        let sender = lock(&self.live)
            .get_mut(&run_id)
            .and_then(|entry| entry.cancel.take());
        match sender {
            Some(tx) => tx.send(why).await.is_ok(),
            None => false,
        }
    }

    /// Discard a finished run's worktree.
    ///
    /// Only for terminal runs, and never automatic: when an agent finishes, its worktree
    /// holds the work — edited files, commits on the run branch. Reclaiming that disk the
    /// moment the process exits would throw away the output before anyone read it. The
    /// branch survives regardless; this removes only the checkout.
    pub async fn cleanup(&self, run_id: RunId) -> Result<Removal, SubmitError> {
        // **Before the load, not after it.** Two handoffs recorded that this guard was already
        // beside its effect — the run was loaded and `remove` called with no `await` between —
        // and taking a lock *is* an await, so loading first would put the check back on the
        // wrong side of one. In this order the terminal test is read inside the hold, which is
        // what makes it true at the moment the directory goes: `Failed` is terminal *and*
        // resumable, so `offload rm` and `offload resume` on the same run are two operators
        // one keystroke apart. Waiting rather than refusing, because whoever is rebuilding
        // will finish, and then this reloads and says "still active" — the sentence that was
        // already right (ADR-0052).
        let held = self.workspaces.hold(run_id).await;
        let run = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| SubmitError::NoSuchRun(run_id.to_string()))?;
        if !run.state.is_terminal() {
            return Err(SubmitError::Agent(format!(
                "run {} is still active; cancel it first",
                run_id.short()
            )));
        }
        // Recovered from the spec rather than remembered: after a restart there is no
        // in-memory record, and cleanup must still work.
        let source = RepoSource::parse(run.spec.workspace().map_or("", |w| w.repo.as_str()));

        // The note only where the disk changed. A worktree lives on one machine and a finished
        // run names none, so a node that removed nothing has nothing to say — and saying it
        // anyway is not merely wrong locally: these numbers gossip, `accept_progress` breaks a
        // tie on the author's clock, and a copy that matches the holder's counters and carries a
        // later stamp wins the whole fleet. `offload rm` on the laptop would tell every node
        // that the desktop's checkout was gone, while it sat there untouched.
        let removal = held.remove(&source).await?;
        if matches!(removal, Removal::Removed { .. }) {
            self.note_workspace(run_id, "removed");
        }
        Ok(removal)
    }

    /// Reclaim a finished occurrence's checkout, if it is here and holds nothing.
    ///
    /// The half of `cleanup` that can happen with nobody watching, and it exists because
    /// ADR-0020 broke the premise `cleanup` was built on. "Never automatic" is right for a run a
    /// person submitted: they will look at the output, and reclaiming the disk the moment the
    /// process exits would throw it away before they did. A **triggered** run has no such person
    /// by construction — that is what unattended means — so the same rule leaves a checkout per
    /// firing on the disk for ever. Measured while walking this feature: a watcher ticking every
    /// three seconds left eight worktrees in forty-five seconds, and nothing in the product would
    /// ever have removed one.
    ///
    /// Three things keep it from being the automatic teardown `cleanup` refuses to be:
    ///
    /// * **Only when the checkout holds nothing.** `holds_uncommitted` asks git, and anything
    ///   uncommitted keeps the whole worktree — the valuable part is the part nothing else has a
    ///   copy of (ADR-0003), and committed work is on the branch either way.
    /// * **Only where the checkout is.** A rule's occurrence may have been placed on a peer
    ///   (ADR-0006), and a node reclaiming a worktree it never had would report a removal it did
    ///   not perform — into numbers that gossip and win on the author's clock. `None` from
    ///   `holds_uncommitted` and `Removal::NothingHere` are the two halves of not answering.
    /// * **Only for a terminal run**, which `cleanup` enforces and the caller has already
    ///   established by a different route: a rule fires its next occurrence only once the last
    ///   one has finished.
    pub async fn reclaim_occurrence(&self, run_id: RunId) -> Removal {
        match self.workspaces.holds_uncommitted(run_id).await {
            Some(false) => {}
            Some(true) => {
                tracing::debug!(
                    run_id = %run_id,
                    "keeping this occurrence's checkout: it has uncommitted work in it"
                );
                return Removal::NothingHere;
            }
            None => return Removal::NothingHere,
        }
        match self.cleanup(run_id).await {
            Ok(removal) => removal,
            Err(e) => {
                tracing::debug!(run_id = %run_id, error = %e, "could not reclaim a checkout");
                Removal::NothingHere
            }
        }
    }

    /// Reclaim the checkouts of runs that are not this node's any more (ADR-0023).
    ///
    /// The third place `cleanup`'s premise does not hold, and the first one that is about a
    /// machine doing no work at all. `cleanup` is manual because somebody will read the worktree;
    /// ADR-0020 §6 found the path where a *rule's own* occurrence has nobody, and reclaimed it
    /// where the rule is. Two checkouts it cannot reach:
    ///
    /// * one a run **left**, because nothing removes a worktree when a run moves — `remove` has
    ///   exactly one production caller and it is terminal-only. The work went with the run: the
    ///   checkpoint travelled and committed work is on the branch in the mirror.
    /// * one an occurrence was **placed on a peer**. ADR-0021 reclaims only where the checkout is,
    ///   and on the peer there is no rule, no firing, and nothing that will ever ask. That is the
    ///   same runaway the records had, one directory bigger.
    ///
    /// Five guards, and each is a rule from somewhere else:
    ///
    /// * **We know the run.** Unknown is not none (the collector's rule), and for a checkout the
    ///   safe answer to not knowing is to keep it.
    /// * **We are not its holder.** A run that is ours is one we may be about to resume.
    /// * **No agent of ours is live on it.** The one that `holds` cannot answer: a leg that has
    ///   just been superseded is no longer the holder and its process is still being taken down,
    ///   and that process is writing into this very directory.
    /// * **Another leg produced the run's latest position.** The guard that says the run has
    ///   *moved on*, and the reason it is not [`Self::describes`]: a finished run has **no
    ///   holder** — the lease goes with the terminal transition — so a holder check alone calls
    ///   every machine that ever finished a run a stranger and reclaims the checkout somebody was
    ///   going to read. `describes` answers that correctly and only for *this incarnation*, since
    ///   it reads the in-memory `live` map, so after a restart it says no about every run on the
    ///   disk. `RunProgress::by` is the durable form of the same fact — the leg that last wrote
    ///   the run's position is by construction the leg that ran it (session twenty-three's
    ///   `offload logs` fallback rests on exactly this) — and an *unstamped* record is unknown
    ///   rather than somebody else, so it keeps the checkout.
    /// * **Nothing uncommitted in it.** ADR-0020 §6's bound verbatim and for its reason — asked
    ///   of git, because the summary beside the run is a turn boundary stale by construction and
    ///   this is the question whose wrong answer deletes somebody's work.
    ///
    /// **An occurrence that finished on a peer *is* reached**, by the second door. This comment
    /// said the opposite for a session — "that peer ran the last leg, so its checkout looks
    /// exactly like an ordinary run somebody is about to read, and nothing travelling tells it
    /// otherwise" — which was true of ADR-0023 and stopped being true two commits later, when
    /// ADR-0024 made exactly that fact travel. `nobody_waiting` is four lines below it. Walked to
    /// settle it: a rule on alpha, alpha refusing to host, every occurrence placed on beta and
    /// finished there — beta's worktree count **oscillates between 0 and 5** over five minutes of
    /// firing every three seconds, where before ADR-0024 it grew one per firing without limit.
    ///
    /// What is genuinely not reached is an operator run that finished on a peer, and that is
    /// `cleanup`'s premise rather than a gap: somebody submitted it, so somebody may read it.
    ///
    /// **Nothing is written down about it beside the run**, and that half has not changed:
    /// `cleanup` notes `workspace: "removed"`, and this must not, because a leg that has lost a
    /// run may not say what its worktree holds — those numbers gossip and win a tie on the
    /// author's clock. That is session seventeen's `offload rm` bug, and it is worse here for
    /// ADR-0021's reason: there was a person typing `rm`, and this runs by itself.
    ///
    /// **It is written down somewhere, though**, which for a session it was not. The residual
    /// that left was read as "the record cannot carry this, so nothing can" — true of the record
    /// and of every field on it, and false of the machine: what happened to a directory is a fact
    /// about *one disk*, and the per-node log is where a fact with no second opinion goes
    /// (`AuditEvent::Reclaimed`, and see [`Self::note_reclaimed`]). `offload audit <run>` is
    /// where somebody asks what became of a worktree that is not there any more; a
    /// `tracing::info!` on an unattended machine was the answer for as long as the fact was
    /// looking for a home on the record.
    ///
    /// **The four guards about the run are taken twice**, and the second time is the one that
    /// decides — see [`Self::reclaimable`]. Everything between them is a `git status` subprocess,
    /// and a run can be picked back up inside it.
    pub async fn reclaim_departed_checkouts(&self) -> usize {
        let mut removed = 0;
        for run_id in self.workspaces.checkouts() {
            // Asked here only to decide whether the git call below is worth making at all: the
            // sweep walks every checkout on the disk and most of them belong to runs it will
            // keep, so asking git about each one would make the cheap answer the expensive one.
            if self.reclaimable(run_id).is_none() {
                continue;
            }
            match self.workspaces.holds_uncommitted(run_id).await {
                Some(false) => {}
                Some(true) => {
                    tracing::debug!(
                        run_id = %run_id,
                        "keeping a departed run's checkout: it has uncommitted work in it"
                    );
                    continue;
                }
                None => continue,
            }
            // **Take the checkout before asking again.** `try_hold` rather than `hold`: a
            // checkout somebody is rebuilding is exactly one this must leave alone, and the
            // next tick is soon enough (ADR-0052). Waiting would also stall the whole sweep
            // behind one bundle fetch.
            let Some(held) = self.workspaces.try_hold(run_id) else {
                tracing::debug!(
                    run_id = %run_id,
                    "not reclaiming this checkout: something else is building or tearing it down"
                );
                continue;
            };
            // **Asked again, on this side of the await, because the answer above is stale.**
            // `Supervisor::resume` — a person typing `offload resume`, or the recovery tick
            // picking an unattended failure back up — assigns the run and registers it with two
            // synchronous calls and no await between them, so a run that was `Failed`, unheld and
            // idle when the guards above ran can be held by this node with an agent being
            // launched into this very directory by the time they are acted on. The effect here is
            // `git worktree remove --force`, which is why this one is not a lost update but
            // somebody's work. Measured before the second call existed: **40 of 40** attempts
            // reclaimed the checkout out from under a run resumed one millisecond into the
            // `git status` — not a photo finish, a window it loses every time.
            //
            // In this order the pair is closed rather than narrowed: a rebuild that has started
            // holds the checkout, so `try_hold` above refused; one that has not started yet has
            // already *registered* the run (`register` before `launch`, ADR-0051), so this call
            // sees the live agent. There is no third moment left for it to arrive in.
            let Some((run, why)) = self.reclaimable(run_id) else {
                tracing::debug!(
                    run_id = %run_id,
                    "not reclaiming this checkout: the run was picked back up while git was asked about it"
                );
                continue;
            };
            let source = RepoSource::parse(run.spec.workspace().map_or("", |w| w.repo.as_str()));
            match held.remove(&source).await {
                // `discarded` is `None` by construction here: `reclaimable` has already
                // refused a checkout holding anything uncommitted, which is the door this sweep
                // is built on. Named rather than ignored, so a future `Some` is a compile error
                // and not a silent reclamation of somebody's only copy.
                Ok(Removal::Removed { discarded: None }) => {
                    tracing::info!(
                        run_id = %run_id,
                        "reclaimed the checkout of a run this node is not running"
                    );
                    // …and durably, on the one machine entitled to say it. Only on `Removed`:
                    // `NothingHere` is a directory that was already gone, and a log of things
                    // that did not happen is the same over-claim the record was kept safe from.
                    self.note_reclaimed(run_id, why);
                    removed += 1;
                }
                Ok(Removal::NothingHere) => {}
                // The guard above answered "clean" and the removal found otherwise: something
                // was written into the checkout between the two. The directory is gone either
                // way — `remove` has already run — so this is a record of what went, on the one
                // machine entitled to state it, rather than a decision.
                Ok(Removal::Removed {
                    discarded: Some(held),
                }) => {
                    tracing::warn!(
                        run_id = %run_id,
                        discarded = %held,
                        "reclaimed a checkout that had gained uncommitted work since it was checked"
                    );
                    self.note_reclaimed(run_id, why);
                    removed += 1;
                }
                Err(e) => tracing::debug!(run_id = %run_id, error = %e, "could not reclaim"),
            }
        }
        removed
    }

    /// Every door in front of reclaiming a checkout, asked as one question.
    ///
    /// Its own function because [`Self::reclaim_departed_checkouts`] asks it **twice**: once to
    /// decide whether `holds_uncommitted` is worth a subprocess, and once again immediately
    /// before the removal, where the answer is the one that decides. Two copies of this would be
    /// two chances to disagree about what "reclaimable" means, and the one on the far side of the
    /// await is the one nobody would remember to update — the same reasoning, and the same shape,
    /// as `refuse_unresumable` either side of `Store::update_run`'s lock.
    ///
    /// Returns the run, because the caller needs its spec to name the repo, and because a
    /// predicate that answered `bool` would send the caller back to the store for a *third*,
    /// differently-timed copy of it.
    ///
    /// And **which of the two doors it went through**, for the same reason and one step further:
    /// the removal is written down now (`AuditEvent::Reclaimed`), and the only place that knows
    /// whether this checkout went because the run *moved* or because nobody was coming to read it
    /// is the function that asked. A caller re-deriving it from the run afterwards would be a
    /// second copy of this rule, read at a third moment, disagreeing with the decision it is
    /// describing — which is what `explain` exists not to be.
    ///
    /// The four guards, each a rule from somewhere else and each documented at the caller:
    /// we know the run, we are not its holder, no agent of ours is live on it, and either it has
    /// moved on or nobody was ever going to read it.
    fn reclaimable(&self, run_id: RunId) -> Option<(Run, offload_core::Reclamation)> {
        let run = self.run(run_id)?;
        // `live` is *not* a map of running agents — nothing removes an entry from it, so a
        // key here means "this incarnation started that run at some point", which is true of
        // every occurrence a peer has ever hosted. Found by walking it: with `contains_key`
        // the sweep reclaimed nothing at all on the one machine it was written for, and the
        // unit test could not see it because a fixture's runs are never actually run.
        // `cancel` is the handle to the process, and `release` clears it when the agent is
        // gone — the same field `record_run` reads to tell a live leg from a remembered one.
        let agent_here = lock(&self.live)
            .get(&run_id)
            .is_some_and(|entry| entry.cancel.is_some());
        if run.holder() == Some(self.node_id) || agent_here {
            return None;
        }
        // Two doors, and a checkout goes through either. **Has the run moved on** — only
        // another leg's position says so durably. Or **was nobody ever going to read it**:
        // a terminal machine-started run is the case ADR-0020 §6 found and ADR-0024 made
        // legible to a node that has never heard of the rule, which is the node that has
        // been keeping one of these per firing.
        let moved_on = self
            .store
            .load_stats(run_id)
            .ok()
            .and_then(|stats| stats.by)
            .is_some_and(|by| by != self.node_id);
        let nobody_waiting = run.origin == offload_core::Origin::Rule && run.state.is_terminal();
        // `moved_on` first where both are true, because it is the stronger statement about where
        // the work is: the run is on another machine now, whoever submitted it. `nobody_waiting`
        // is the weaker one — it says only that no person is coming — and reporting that about a
        // run somebody else is *running* would be true and beside the point.
        match (moved_on, nobody_waiting) {
            (true, _) => Some((run, offload_core::Reclamation::MovedOn)),
            (false, true) => Some((run, offload_core::Reclamation::NobodyWaiting)),
            (false, false) => None,
        }
    }

    /// Checkouts on this disk that no run record here accounts for, and where they are.
    ///
    /// ADR-0023's last stated residual, which was left correct and silent: the sweep keeps a
    /// checkout whose run it has never heard of, on the sound grounds that the safe answer to not
    /// knowing is to keep it — and then nothing said the directory was there. It is reachable
    /// rather than theoretical: prune a record (ADR-0021) whose checkout could not be reclaimed
    /// because it held **uncommitted work**, and that directory is the only copy of that work and
    /// nothing will ever look at it again.
    ///
    /// A count and a path, not a size. `offload status` is asked often and walking every worktree
    /// to add up bytes would make the cheapest command in the CLI the one that touches the most
    /// disk; the path is what makes `du -sh` the obvious next thing to type.
    ///
    /// "No record" means exactly that — not "not ours". A checkout for a run somebody else holds
    /// is ordinary and is the sweep's business, and counting it here would report every migration
    /// in progress as a stray directory.
    #[must_use]
    pub fn stray_checkouts(&self) -> u64 {
        self.workspaces
            .checkouts()
            .into_iter()
            .filter(|id| self.run(*id).is_none())
            .count() as u64
    }

    /// This node has been drained: bid on nothing more, accept nothing more.
    ///
    /// Called by the drain, which is the graceful departure. Not undone — see the field.
    pub fn stop_accepting(&self) {
        self.draining
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// This node has been revoked: stop every agent it is running, and start no more (ADR-0044).
    ///
    /// The one machine that can stop this node's duplicate legs is this node. The fleet has
    /// already evicted it and moved its runs — that much is the partition ADR-0002 accepts, and
    /// the two agents it leaves on one repository are the failure mode `CLAUDE.md` names first.
    /// Everything the ordinary supersede path relies on is gone here: a revoked node hears no
    /// gossip, so no new holder's record arrives, so `record_run` never fires and the old leg
    /// runs to the end. What it *does* still receive is the refusal on every dial it makes, and
    /// ADR-0044 is what makes that refusal carry a fact this node can check for itself.
    ///
    /// Idempotent by construction rather than by a flag: `halt_agent` takes the cancel sender,
    /// so a second pass finds nothing to signal. The latch is read to decide whether there is
    /// anything to *say*, which is the only part that would otherwise repeat once a second.
    ///
    /// Returns the runs it stopped, and `None` when this node had already stood down.
    pub async fn stand_down(&self) -> Option<Vec<RunId>> {
        if self.revoked.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        // Both latches. A revoked node is also a node that takes no work, and every door that
        // already asks `is_draining` — the bid, the grant, `offload status` — is a door that has
        // to be shut here too. `stop_accepting` is not *why* it refuses, which is why the status
        // line asks this one first.
        self.stop_accepting();
        // Only the entries that still hold a handle, for `cancel_all`'s reason: an entry outlives
        // its run, so the keys of this map are every run this daemon has started.
        let ids: Vec<RunId> = lock(&self.live)
            .iter()
            .filter(|(_, entry)| entry.cancel.is_some())
            .map(|(id, _)| *id)
            .collect();
        let mut stopped = Vec::new();
        for id in ids {
            if self.halt_agent(id, Halt::Revoked).await {
                stopped.push(id);
            }
        }
        Some(stopped)
    }

    /// Has this node been revoked from its fleet?
    ///
    /// The supervisor's copy of a fact that lives in `fleet.json`, latched here because this is
    /// where the runs are and because it must not un-set: the file is rewritten by commands that
    /// need no daemon, and a node that stopped its agents and then read a file mid-rename would
    /// otherwise start them again.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Stop every run. Used on daemon shutdown.
    ///
    /// The process half only: this is the daemon going away, so what matters is that no agent
    /// outlives it. The records are left alone — a shutdown is not somebody deciding the work
    /// was unwanted, and phase 4's drain is what hands these runs on rather than ending them.
    pub async fn cancel_all(&self) {
        // Only the entries that still hold a handle. An entry outlives its run on purpose — see
        // `close_stream` — so the keys of this map are every run this daemon has ever started,
        // and `halt_agent` would answer `false` for all but a few of them. Filtering here makes
        // the shutdown pass proportional to what is running rather than to what has run.
        let ids: Vec<RunId> = lock(&self.live)
            .iter()
            .filter(|(_, entry)| entry.cancel.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            // `Superseded` rather than a cancel, for its reason read the other way: the run is
            // not over, this daemon is. Writing it terminal on the way out would mean a restart
            // finding work somebody had never decided to abandon.
            self.halt_agent(id, Halt::Superseded).await;
        }
    }
}

/// Is this run in a state a resume can act on at all?
///
/// Its own function because it is asked **twice** — once on the caller's copy, for the refusals a
/// person reads, and once inside `Store::update_run`'s lock, where the answer is the one that
/// decides. Two copies of this match would be two chances to disagree about what "resumable"
/// means, and the one inside the lock is the one nobody would remember to update.
/// The states `offload resume` will continue a run from.
///
/// Split out of [`refuse_unresumable`] so that a *report* can ask the same question the door
/// asks. The door needs the other arms too — it has to say which kind of no it is — but a
/// report only needs to know whether `offload resume` is the next thing somebody would type at
/// this run, which is the gate on saying whether this node would take it. Two spellings of that
/// list is how a listing comes to offer advice about a run the command would refuse anyway, and
/// this file's history is mostly made of the same mistake one layer up.
#[must_use]
pub fn resumable_state(run: &Run) -> bool {
    // `Pending` is a run released by `offload checkpoint`; `Failed` is one interrupted.
    matches!(
        run.state,
        RunState::Pending { .. } | RunState::Failed { .. }
    )
}

/// Is `offload resume` the next thing a **person** would type at this run?
///
/// [`resumable_state`] is the *door's* question, and it is deliberately tier-blind: a task's own
/// door, [`Supervisor::restart_task`], admits a run by the same states, because what a run has to
/// be for either of them to reopen it is a property of the run and two copies of that match would
/// be two chances to disagree. This is the *report's* question, and it has a second half the
/// door's does not: **a task is never resumed** (ADR-0058). There is no conversation to continue,
/// so `offload resume` refuses every task on every node for ever and the recovery tick runs the
/// program again instead — which is why `restart_task` exists as a separate door with its own
/// verb.
///
/// Measured on two daemons before this existed: a failed task on a node without `host-runs` was
/// given the whole resume pairing by two reports at once — `offload ps`'s footnote (*"a run above
/// says it is resumable, which is true of the run"*, which for a task is false) and `offload
/// explain`'s *"resume refused here right now: this node has not been granted host-runs — run
/// `offload grant host-runs`"*. Both named a **fleet grant** as the fix for a command that would
/// have refused on the node that had it, with a different and permanent reason: `offload resume`
/// on the same run there answered *"it has no checkpoint — there is no conversation to
/// continue"*. A node-level, temporary-sounding refusal standing in front of a run-level,
/// permanent one.
#[must_use]
pub fn resumable_by_hand(run: &Run) -> bool {
    resumable_state(run) && run.spec.work.kind() == offload_core::WorkKind::Agent
}

/// `action` is the verb the refusal is about — "resumed" for a conversation, "restarted" for a
/// task (ADR-0058). The *states* are the same question for both, which is why this is one
/// function: what a run has to be for either door to reopen it is a property of the run, and two
/// copies of this match would be two chances to disagree about it.
fn refuse_unresumable(run: &Run, run_id: RunId, action: &'static str) -> Result<(), SubmitError> {
    if resumable_state(run) {
        return Ok(());
    }
    match run.state {
        // Finished on purpose. Reopening one would restart work somebody deliberately stopped —
        // and "it finished" is a more useful sentence than "it is busy".
        RunState::Completed { .. } | RunState::Cancelled { .. } => Err(SubmitError::Refused {
            run: run_id.short(),
            action,
            reason: format!(
                "it {} — start a new run instead",
                run.state.name().replace("completed", "finished")
            ),
        }),
        ref other => Err(SubmitError::Refused {
            run: run_id.short(),
            action,
            reason: format!("it is {}, so somebody is already holding it", other.name()),
        }),
    }
}

/// Shift an event's turn numbers so they continue the run rather than the process.
///
/// Every resume starts a new agent process, and a new agent process counts from one. The
/// run has been going the whole time, so the two counts have to be reconciled somewhere —
/// here, at the point events enter the daemon, rather than in each of the four places that
/// go on to read a turn number.
fn accumulate_turns(event: AgentEvent, offset: u32) -> AgentEvent {
    if offset == 0 {
        return event;
    }
    match event {
        AgentEvent::TurnBoundary { turn } => AgentEvent::TurnBoundary {
            turn: turn.saturating_add(offset),
        },
        AgentEvent::Finished(mut outcome) => {
            outcome.turns = outcome.turns.saturating_add(offset);
            AgentEvent::Finished(outcome)
        }
        other => other,
    }
}

/// The reason a run failed, if it did. Read off the state machine rather than tracked
/// alongside it, so the two cannot disagree.
fn failure_reason(run: &Run) -> Option<String> {
    match &run.state {
        RunState::Failed { reason, .. } => Some(reason.clone()),
        _ => None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        return flat;
    }
    let kept: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// This leg's spend entry in a run's numbers, begun at `base` if it has none yet (ADR-0067 §1).
///
/// **Called only where a leg starts**, which is the one place that knows the base. Every other
/// writer updates an existing entry and never creates one (see `update_stats`). Begun once and never
/// re-based: a leg resumed again under the same epoch (a daemon that restarted and reclaimed its
/// run) keeps the base it started with, which is what it did not spend.
fn begin_leg(
    stats: &mut offload_core::RunProgress,
    by: offload_core::NodeId,
    epoch: offload_core::run::Epoch,
    base: offload_core::TokenUse,
) -> &mut offload_core::LegSpend {
    if !stats.legs.iter().any(|l| l.by == by && l.epoch == epoch) {
        stats.absorb_leg(offload_core::LegSpend {
            by,
            epoch,
            base,
            tokens: base,
            cost_micro_usd: 0,
        });
    }
    let position = stats
        .legs
        .iter()
        .position(|l| l.by == by && l.epoch == epoch)
        .unwrap_or(0);
    &mut stats.legs[position]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0064 §3: the continuer's prompt first, then the parent's two strings **verbatim**
    /// under one heading — and the absence of either said in words rather than left as a gap.
    #[test]
    fn a_handoff_carries_the_parents_words_verbatim_and_names_what_is_missing() {
        let parent = RunId::from_bytes([7; 16]);
        let full = handoff_prompt(
            "now add tests",
            parent,
            "fix the parser\nand the lexer",
            Some("Done: parser fixed."),
            true,
        );
        assert!(full.starts_with("now add tests\n"), "{full}");
        assert!(
            full.contains(&parent.to_string()),
            "the parent whole: {full}"
        );
        assert!(full.contains("> fix the parser\n> and the lexer"), "{full}");
        assert!(full.contains("> Done: parser fixed."), "{full}");
        assert!(full.contains(offload_core::PARENT_TRANSCRIPT), "{full}");

        let thin = handoff_prompt("now add tests", parent, "fix it", None, false);
        assert!(thin.contains("It ended with no closing message."), "{thin}");
        assert!(thin.contains("could not be found"), "{thin}");
        assert!(!thin.contains(offload_core::PARENT_TRANSCRIPT), "{thin}");
    }

    /// A continuation typed on one node, with its base built on another, is **this** node's run.
    ///
    /// The point of forwarding only the base: the run's home, and what `--prefer here` resolves
    /// to, are the machine the person typed at — not the one that held the parent's checkout,
    /// which is where the base came from. And what the base carries arrives unchanged, because
    /// the start path recognises it by its turn count alone.
    #[test]
    fn a_continuation_made_from_a_peers_base_is_the_asking_nodes_run() {
        let asker = NodeId::from_bytes([2; 32]);
        let builder = NodeId::from_bytes([9; 32]);
        let cfg = Config {
            state_dir: std::env::temp_dir().join(format!("offload-sup-{}", std::process::id())),
            ..Config::default()
        };
        let sup = Supervisor::new(Arc::new(cfg), asker, Store::open_memory().expect("store"))
            .with_private_ledger();

        // The parent as the asker knows it: the fleet's gossiped copy, finished elsewhere.
        let parent_id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut parent = Run::new(
            parent_id,
            spec(&RepoSource::parse("/tmp/whatever")),
            builder,
            now(),
        );
        parent.state = RunState::Completed { at: now() };

        let mut at_zero = checkpoint_at(0);
        at_zero.session_id = None;
        at_zero.bundle = Some(offload_core::BlobHash::from_bytes([4; 32]));
        let base = offload_core::ContinuationBase {
            checkpoint: at_zero.clone(),
            transcript: Some(offload_core::BlobHash::from_bytes([5; 32])),
            closing_message: Some("Done: parser fixed.".into()),
            summary: "1 commit(s)".into(),
        };
        let req = crate::api::ContinueRequest {
            run: parent_id.short(),
            prompt: "now add tests".into(),
            mode: offload_core::ContinueMode::Handoff,
            queue: false,
            deadline: None,
            allow: Vec::new(),
            max_turns: None,
            ask: offload_core::AskPolicy::default(),
            require: None,
            prefer: Some(offload_core::Wanted {
                clauses: Constraint::Always,
                nodes: vec![offload_core::NodeRef::Here],
            }),
            hold_until: None,
        };

        let run = sup
            .continuation_from(req, &parent, base, &[])
            .expect("a continuation");
        assert_eq!(run.home, asker);
        assert_eq!(
            run.spec.prefer.named_nodes(),
            vec![asker],
            "here is the asker"
        );
        assert_eq!(run.continuation_base(), Some(&at_zero));
        let continues = run.spec.parent.as_ref().expect("a parent");
        assert_eq!(continues.run, parent_id);
        assert_eq!(
            continues.transcript,
            Some(offload_core::BlobHash::from_bytes([5; 32]))
        );
        assert_eq!(
            continues.closing_message.as_deref(),
            Some("Done: parser fixed.")
        );
        assert_eq!(
            run.spec.agent().map(|a| a.prompt.as_str()),
            Some("now add tests")
        );
    }

    fn supervisor_with(store: Store) -> Supervisor {
        let cfg = Config {
            state_dir: std::env::temp_dir().join(format!("offload-sup-{}", std::process::id())),
            ..Config::default()
        };
        Supervisor::new(Arc::new(cfg), NodeId::from_bytes([1; 32]), store).with_private_ledger()
    }

    fn supervisor() -> Supervisor {
        supervisor_with(Store::open_memory().expect("store"))
    }

    /// A run accepted here and not started must not be told it is "already under way".
    ///
    /// The pre-fix behaviour is `edit_spec` answering `pending?` with two arms, so every
    /// non-pending run — including one `offload ps` calls *waiting for a slot* — got the
    /// running text. Measured on one daemon capped at one slot: a deadline put on the older of
    /// two queued runs sent it from first to **last**, and the note said the change affected
    /// "only how long the fleet waits for it if something goes wrong".
    #[test]
    fn a_queued_run_is_not_told_it_is_already_under_way() {
        use offload_core::SpecEdit;

        // Front of the queue: no caveat to add, and it says which run goes next.
        let first = queue_note(SpecEdit::Priority { to: 10 }, 0, 3);
        assert!(first.contains("has not started yet"), "{first}");
        assert!(first.contains("first of the 3"), "{first}");
        assert!(!first.contains("under way"), "{first}");

        // Behind something: the operator has to be told their edit moved it past nothing, or
        // they have typed a command that did nothing and been congratulated for it.
        let behind = queue_note(SpecEdit::Priority { to: 10 }, 1, 3);
        assert!(behind.contains("2nd of the 3"), "{behind}");
        assert!(behind.contains("urgency leads"), "{behind}");
        assert!(!behind.contains("under way"), "{behind}");

        // The deadline's surprise is the direction: a stated time sorts *behind* every run that
        // said "as soon as you can", which is what the walk measured.
        let later = queue_note(SpecEdit::Deadline { at: None }, 2, 3);
        assert!(later.contains("3rd of the 3"), "{later}");
        assert!(later.contains("as soon as it can be"), "{later}");
        assert!(!later.contains("under way"), "{later}");

        // Neither edit may claim a run has begun, in any position.
        for at in 0..4 {
            for edit in [
                SpecEdit::Priority { to: 1 },
                SpecEdit::Deadline { at: None },
            ] {
                let note = queue_note(edit, at, 4);
                assert!(!note.contains("under way"), "{note}");
                assert!(note.contains("has not started yet"), "{note}");
            }
        }
    }

    #[test]
    fn a_position_in_a_queue_is_spelled_the_way_somebody_reads_it() {
        for (n, want) in [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (4, "4th"),
            (11, "11th"),
            (12, "12th"),
            (13, "13th"),
            (21, "21st"),
            (22, "22nd"),
            (23, "23rd"),
            (101, "101st"),
            (111, "111th"),
        ] {
            assert_eq!(ordinal(n), want, "{n}");
        }
    }

    /// ADR-0023's four guards, over real worktrees, because this function deletes directories.
    ///
    /// The three that must survive a sweep are each a different reason, and a version that got
    /// any one of them wrong would look fine on the other two: a run **we hold** may be about to
    /// resume, a checkout with **uncommitted work** holds the only copy of it, and a run we have
    /// **no record of** is the collector's unknown-is-not-none read in the direction where the
    /// safe answer is to keep.
    #[tokio::test]
    async fn a_departed_checkout_goes_and_the_three_that_must_not_do_not() {
        let dir = std::env::temp_dir().join(format!(
            "offload-departed-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().elapsed().map(|d| d.as_nanos())
        ));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "t@offload.local"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        std::fs::write(src.join("README.md"), "hi\n").expect("write");
        for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", "i"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        let source = RepoSource::Local(src.clone());

        let store = Store::open_memory().expect("store");
        let cfg = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        let sup = Supervisor::new(Arc::new(cfg), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();
        let peer = NodeId::from_bytes([2; 32]);

        // Four checkouts, four situations.
        let mut ids = Vec::new();
        for tag in 0..4u8 {
            let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
            sup.workspaces
                .hold(id)
                .await
                .prepare(&source, None, None)
                .await
                .expect("prepare");
            ids.push((tag, id));
        }
        let (departed, held, dirty, unknown) = (ids[0].1, ids[1].1, ids[2].1, ids[3].1);

        for (id, holder) in [(departed, peer), (held, sup.node_id), (dirty, peer)] {
            let mut run = Run::new(id, spec(&source), sup.node_id, now());
            let epoch = run
                .assign(holder, now(), offload_core::LEASE)
                .expect("assign");
            run.started(holder, epoch, now()).expect("start");
            store.save_run(&run).expect("save");
            // The run's latest position came from the leg running it, which is the durable form
            // of "this run has moved on". An unstamped record is *unknown*, not somebody else.
            store
                .save_stats(
                    id,
                    &offload_core::RunProgress {
                        by: Some(holder),
                        ..Default::default()
                    },
                )
                .expect("stats");
        }
        // …and `unknown` gets no record at all.
        std::fs::write(
            sup.workspaces.worktree_path(dirty).join("mid_turn.rs"),
            "fn never_captured(\n",
        )
        .expect("write");

        assert_eq!(sup.reclaim_departed_checkouts().await, 1);

        let left = sup.workspaces.checkouts();
        assert!(
            !left.contains(&departed),
            "a clean checkout of a run running elsewhere is the whole point"
        );
        assert!(left.contains(&held), "a run we hold may be about to resume");
        assert!(
            left.contains(&dirty),
            "uncommitted work is the only copy of itself"
        );
        assert!(
            left.contains(&unknown),
            "a run we have no record of is one we must not decide about"
        );
        // …and the residual ADR-0023 stated and left silent. Keeping it is right; nothing saying
        // it is there is what made it a leak nobody could find. `offload status` prints this, and
        // it must count only the record-less one — the held and the dirty checkouts have records
        // and are the sweep's ordinary business, so counting them would report every migration in
        // progress as a stray directory.
        assert_eq!(
            sup.stray_checkouts(),
            1,
            "one checkout with no record, and the other three are accounted for"
        );

        // `live` is not a map of running agents, and a fixture's runs are never actually run —
        // so without this the sweep passed every test and reclaimed **nothing at all** on the one
        // machine it was written for. Nothing removes an entry from `live`; `release` clears the
        // process handle, which is what "an agent is here" actually means.
        let ran_here = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(ran_here)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut done = Run::new(ran_here, spec(&source), sup.node_id, now())
            .started_by(offload_core::Origin::Rule);
        done.state = offload_core::RunState::Completed { at: now() };
        store.save_run(&done).expect("save");
        lock(&sup.live).insert(
            ran_here,
            LiveRun {
                epoch: done.epoch,
                live: Some(tokio::sync::broadcast::channel(4).0),
                // The agent is gone: this is what `release` leaves behind, and it is the state
                // every finished run on a long-lived daemon is in.
                cancel: None,
                checkpoint_requested: None,
            },
        );
        assert_eq!(
            sup.reclaim_departed_checkouts().await,
            1,
            "a remembered run is not a running one"
        );

        // ADR-0024's second door: a terminal machine-started run's checkout goes whoever ran it,
        // because nobody was ever going to read that output. The measured case is a peer hosting
        // a rule's occurrences — 62 firings, 62 worktrees, 145 MB in five minutes — where the
        // `by` guard above says "this machine ran the last leg" and is exactly right about the
        // wrong question.
        let occurrence = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(occurrence)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut fired = Run::new(occurrence, spec(&source), sup.node_id, now())
            .started_by(offload_core::Origin::Rule);
        fired.state = offload_core::RunState::Completed { at: now() };
        store.save_run(&fired).expect("save");
        store
            .save_stats(
                occurrence,
                &offload_core::RunProgress {
                    by: Some(sup.node_id),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert_eq!(
            sup.reclaim_departed_checkouts().await,
            1,
            "the leg that ran it is this one, and that is precisely why nobody will read it"
        );
        assert!(!sup.workspaces.checkouts().contains(&occurrence));

        // Both doors at once, which is the only place the order of the two matters: a rule's
        // occurrence that started here and *finished on a peer* is a run that moved on **and** a
        // run nobody will read. Reachable — it is the sweep's second door reached through the
        // first — and the sentence has to be the stronger one, because "nobody was going to read
        // it" is true and beside the point about a worktree whose work is on another machine.
        // Written down here because otherwise the precedence is a claim about line order, and
        // this file carries two entries about those.
        let both = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(both)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut moved = Run::new(both, spec(&source), sup.node_id, now())
            .started_by(offload_core::Origin::Rule);
        moved.state = offload_core::RunState::Completed { at: now() };
        store.save_run(&moved).expect("save");
        store
            .save_stats(
                both,
                &offload_core::RunProgress {
                    by: Some(peer),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert_eq!(sup.reclaim_departed_checkouts().await, 1);

        // The case a holder check alone gets catastrophically wrong, and the reason this guard
        // is `RunProgress::by` rather than the holder: a run that **finished here** has no
        // holder at all — the lease goes with the terminal transition — so "not the holder" is
        // true of every machine that ever finished a run, and the checkout somebody was going to
        // read would go with it. That is `cleanup`'s whole premise, and it still holds here.
        let mine = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(mine)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut over = Run::new(mine, spec(&source), sup.node_id, now());
        over.state = offload_core::RunState::Completed { at: now() };
        store.save_run(&over).expect("save");
        store
            .save_stats(
                mine,
                &offload_core::RunProgress {
                    by: Some(sup.node_id),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert_eq!(
            sup.reclaim_departed_checkouts().await,
            0,
            "a run that finished on this machine keeps its checkout"
        );
        assert!(sup.workspaces.checkouts().contains(&mine));

        // And nothing was said about any of it *on the record*: these numbers gossip, and a leg
        // that has lost a run may not describe its worktree.
        assert_ne!(
            store.load_stats(departed).expect("stats").workspace,
            "removed"
        );
        // …but the disk changed, and that is this machine's own business to state. The two
        // reclaimed checkouts went through different doors and the log has to tell them apart:
        // `departed` moved to a peer, `occurrence` was a rule's own firing that nobody would
        // read. `mine` finished here and was kept, so there must be no row for it at all — a log
        // that records removals that did not happen is the over-claim the record was kept from.
        let door = |id| {
            store
                .audit(Some(id), 100)
                .expect("audit")
                .into_iter()
                .find_map(|entry| match entry.kind {
                    offload_core::AuditEvent::Reclaimed { why, .. } => Some(why),
                    _ => None,
                })
        };
        assert_eq!(
            door(departed),
            Some(offload_core::Reclamation::MovedOn),
            "the run is on another machine now, and that is what became of this worktree"
        );
        assert_eq!(
            door(occurrence),
            Some(offload_core::Reclamation::NobodyWaiting),
            "the leg that ran it is this one, which is precisely why nobody will read it"
        );
        assert_eq!(
            door(both),
            Some(offload_core::Reclamation::MovedOn),
            "both doors are open, and where the work went is the more useful of the two"
        );
        assert_eq!(
            door(mine),
            None,
            "a checkout that was kept is not a checkout that was reclaimed"
        );
        assert_eq!(
            door(dirty),
            None,
            "and neither is one the sweep refused because of what is in it"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A rule's occurrence that ran somewhere else must not be reported as reclaimed here.
    ///
    /// The narrow half of the bug session seventeen found with `offload rm`: a worktree lives on
    /// one machine, and a node that removed nothing has nothing to say. It matters more here
    /// than there, because there was a person typing `rm` and this fires by itself every time a
    /// rule does — so a wrong answer would gossip a peer's checkout away, unattended, on a
    /// schedule.
    #[tokio::test]
    async fn reclaiming_an_occurrence_that_is_not_here_touches_nothing() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, _run) = dormant_run(&sup, &store);
        assert_eq!(
            sup.reclaim_occurrence(id).await,
            Removal::NothingHere,
            "there is no worktree for this run on this node, so there is nothing to say"
        );
        let stats = store.load_stats(id).expect("stats");
        assert_ne!(
            stats.workspace, "removed",
            "and above all it must not write a claim about another machine's disk — these \
             numbers gossip, and a later stamp wins the fleet"
        );
    }

    /// A checkout the run has been picked back up from must survive the sweep's decision.
    ///
    /// [`Supervisor::reclaim_departed_checkouts`] took its four guards about the *run*, then
    /// awaited the fifth — `holds_uncommitted`, a `git status` subprocess, which is about the
    /// *directory* — and only then removed that directory. Everything the guards read can change
    /// inside that await: `Supervisor::resume` assigns the run and registers it with two
    /// synchronous calls and no await between them, so a run that was `Failed` with no holder when
    /// the sweep looked is held by this node, with an agent being launched into that very
    /// worktree, by the time the sweep acts on what it saw. `Workspaces::remove` runs
    /// `git worktree remove --force`.
    ///
    /// **The window is made wide on purpose, and that took two goes.** The form that found this
    /// raced a 1ms pickup against the 3ms `git status` of a two-file fixture and measured **40 of
    /// 40** before the fix — then went red 2 times in 8 workspace runs afterwards, because under
    /// full-suite parallelism the sleep can outlast the subprocess and the sweep removes the
    /// checkout *correctly*. A guard that fails for the right reason is still a flaky guard, and
    /// flaky guards get deleted.
    ///
    /// Replacing it with a purely deterministic version was worse and is worth recording: asserting
    /// `reclaimable` before and after the pickup passes **with the fix removed**, because the state
    /// has to change *inside* the loop and a test that changes it beforehand only ever exercises
    /// the first call. Watched red-then-green, and it stayed green either way.
    ///
    /// So the window is widened instead of the race being abandoned. `big.bin` is 16 MB, and the
    /// worktree copy is rewritten with identical bytes before each attempt: git's stat cache misses
    /// on the new mtime, so `git status` must re-hash the file to decide it is unchanged — **83ms,
    /// and it still reports the worktree clean**, which is what keeps the sweep on the path that
    /// removes. That is an 83x margin over the 1ms pickup instead of 3x. The deterministic
    /// assertions are kept beside it for what they do cover: that `reclaimable` closes both doors.
    #[tokio::test]
    async fn a_checkout_resumed_mid_sweep_is_not_reclaimed() {
        let dir = std::env::temp_dir().join(format!(
            "offload-midsweep-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().elapsed().map(|d| d.as_nanos())
        ));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "t@offload.local"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        std::fs::write(src.join("README.md"), "hi\n").expect("write");
        // What makes the window wide enough to be raced reliably — see the note above. The bytes
        // repeat, so this costs almost nothing in the object store; what costs is SHA-1 over 16 MB
        // in the worktree, which is what `git status` is made to redo.
        let big: Vec<u8> = (0..16 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        std::fs::write(src.join("big.bin"), &big).expect("write");
        for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", "i"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        let source = RepoSource::Local(src.clone());

        let store = Store::open_memory().expect("store");
        let cfg = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        let sup = Supervisor::new(Arc::new(cfg), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();
        let peer = NodeId::from_bytes([2; 32]);

        for _ in 0..5 {
            let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
            sup.workspaces
                .hold(id)
                .await
                .prepare(&source, None, None)
                .await
                .expect("prepare");

            // What the sweep sees: a run whose last leg was the peer's — so it has *moved on* —
            // that then failed, which leaves it terminal, unheld, and with a clean checkout here.
            // This is the shape ADR-0013 picks back up by itself.
            let mut run = Run::new(id, spec(&source), sup.node_id, now());
            let epoch = run
                .assign(peer, now(), offload_core::LEASE)
                .expect("assign");
            run.started(peer, epoch, now()).expect("start");
            run.fail(peer, epoch, "its agent stopped", now())
                .expect("fail");
            store.save_run(&run).expect("save");
            store
                .save_stats(
                    id,
                    &offload_core::RunProgress {
                        by: Some(peer),
                        ..Default::default()
                    },
                )
                .expect("stats");

            // Identical bytes, new mtime: git's stat cache misses and it must re-hash 16 MB to
            // conclude the worktree is clean. Clean is the point — a dirty checkout is kept by a
            // different guard and would never reach the removal this races.
            std::fs::write(sup.workspaces.worktree_path(id).join("big.bin"), &big).expect("touch");

            // Everything the sweep's first call reads says this checkout may go.
            assert!(
                sup.reclaimable(id).is_some(),
                "the fixture must be one the sweep would act on, or nothing below is a test"
            );

            let sweeper = sup.clone();
            let sweep = tokio::spawn(async move { sweeper.reclaim_departed_checkouts().await });

            // Inside the 83ms `git status`. Exactly the durable half of `resume` — `reopen`,
            // `assign`, `register` — and nothing else, because that is the part that arrives
            // before the launch does, with no await anywhere in it.
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            let picked_up = store
                .update_run(id, |run| {
                    run.reopen(now())?;
                    run.assign(sup.node_id, now(), offload_core::LEASE)
                })
                .expect("update")
                .expect("some")
                .expect("assign");
            let _rx = sup.register(id, picked_up).expect("registered");

            // Both doors the second call closes, checked directly as well, because either alone
            // would let the removal through: the run is ours now, and an agent of ours is live
            // on it.
            assert!(
                sup.reclaimable(id).is_none(),
                "the second call is the one that decides, and it must refuse what the first allowed"
            );

            assert_eq!(sweep.await.expect("sweep"), 0, "it reclaimed something");
            assert!(
                sup.workspaces.checkouts().contains(&id),
                "the checkout was reclaimed out from under a run this node had just picked back up"
            );
            lock(&sup.live).remove(&id);
        }

        // The control, and the reason this test is worth trusting: the same fixture with nobody
        // picking the run back up **is** reclaimed. Without this a `reclaimable` that answered
        // `None` for some unrelated reason would pass everything above by never removing anything,
        // which is the shape of walk that gives the right answer for a reason nobody chose.
        let unraced = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(unraced)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut run = Run::new(unraced, spec(&source), sup.node_id, now());
        let epoch = run
            .assign(peer, now(), offload_core::LEASE)
            .expect("assign");
        run.started(peer, epoch, now()).expect("start");
        run.fail(peer, epoch, "its agent stopped", now())
            .expect("fail");
        store.save_run(&run).expect("save");
        store
            .save_stats(
                unraced,
                &offload_core::RunProgress {
                    by: Some(peer),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert_eq!(
            sup.reclaim_departed_checkouts().await,
            1,
            "the fixture must reach the removal, or the assertions above proved nothing"
        );
        assert!(!sup.workspaces.checkouts().contains(&unraced));

        // And the blast radius, which is wider than the failed run above: a run **parked
        // `Pending` here after migrating** — checkpointed and released, the ordinary state of
        // every run this node has handed on — is reclaimable by the same first door, because its
        // last position came from the other leg and a `Pending` run holds no lease. So the racer
        // is not only the recovery tick on its backoff; it is a person typing `offload resume`,
        // which has no schedule at all.
        let parked = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(parked)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut run = Run::new(parked, spec(&source), sup.node_id, now());
        run.state = offload_core::RunState::Pending {
            since: now(),
            let_go_by: None,
        };
        store.save_run(&run).expect("save");
        store
            .save_stats(
                parked,
                &offload_core::RunProgress {
                    by: Some(peer),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert!(
            sup.reclaimable(parked).is_some(),
            "a run parked here after a migration is reclaimable, so `offload resume` races it too"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The sweep leaves alone a checkout somebody is *building*, and that is a different door
    /// from the one above.
    ///
    /// `reclaimable` asked twice closes the gap between the sweep's guards and its effect. It
    /// cannot close the gap *inside* the effect: `git worktree remove --force` is ~2ms of
    /// somebody else's program, and a rebuild starting inside it adopted a directory that was
    /// deleted before it returned — measured on a forced pair in a scratch repo, 10 of 60, and
    /// every pass that started within 2ms (ADR-0052, and the workspace crate's own
    /// `a_rebuild_never_adopts_a_checkout_a_teardown_is_removing`).
    ///
    /// So the sweep takes the checkout before it asks. Deterministic on purpose, unlike its
    /// neighbour: there is no window to widen, because a held checkout is held until it is
    /// dropped. The control is the same fixture with the hold released, which **is** reclaimed —
    /// without it, a `reclaimable` answering `None` for some unrelated reason would pass the
    /// first half by removing nothing.
    #[tokio::test]
    async fn the_sweep_skips_a_checkout_that_is_being_built() {
        let (dir, source) = scratch_repo("sweep-held");
        let store = Store::open_memory().expect("store");
        let cfg = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        let sup = Supervisor::new(Arc::new(cfg), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();
        let peer = NodeId::from_bytes([2; 32]);
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());

        sup.workspaces
            .hold(id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let epoch = run
            .assign(peer, now(), offload_core::LEASE)
            .expect("assign");
        run.started(peer, epoch, now()).expect("start");
        run.fail(peer, epoch, "its agent stopped", now())
            .expect("fail");
        store.save_run(&run).expect("save");
        store
            .save_stats(
                id,
                &offload_core::RunProgress {
                    by: Some(peer),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert!(
            sup.reclaimable(id).is_some(),
            "the fixture must be one the sweep would act on, or nothing below is a test"
        );

        // What a rebuild holds while it is rebuilding, and nothing else about the run has
        // changed: this is the state the doubled `reclaimable` cannot see, because the rebuild
        // has passed its own registration and is inside a subprocess.
        let held = sup.workspaces.hold(id).await;
        assert_eq!(
            sup.reclaim_departed_checkouts().await,
            0,
            "the sweep tore down a checkout that was being built"
        );
        assert!(sup.workspaces.checkouts().contains(&id));

        drop(held);
        assert_eq!(
            sup.reclaim_departed_checkouts().await,
            1,
            "the fixture must reach the removal, or the assertion above proved nothing"
        );
        assert!(!sup.workspaces.checkouts().contains(&id));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `offload rm` on a leftover checkout must not speak in the winning leg's name.
    ///
    /// Session seventeen's bug was that a node with **no** checkout said "removed" about somebody
    /// else's disk, and the fix was to note only where the disk actually changed. This is the
    /// other half of it, which that fix does not reach: a node that ran an earlier leg still has
    /// its worktree — nothing removes one when a run leaves, which is ADR-0023's whole premise —
    /// so `remove` really does return `Removed` here and the note really is this node's to make.
    ///
    /// What it must not do is make it *as the leg that won*. `update_stats` stamps through
    /// [`Supervisor::writing_leg`], which returns `None` when this node neither has a `live` entry
    /// nor `describes` the run — and `None` deliberately "leaves whatever the row already
    /// carried". That is right for the case its comment names, a run that ended here and whose
    /// `live` entry is long gone, where the stamp on the row is this node's own. After a restart
    /// the same `None` covers the opposite situation: `live` is empty for runs that ended
    /// *elsewhere* too, and the stamp sitting on the row is then the **peer's**, absorbed by
    /// gossip. One return value, two situations, opposite meanings.
    #[tokio::test]
    async fn removing_a_leftover_checkout_does_not_speak_for_the_leg_that_won() {
        let dir = std::env::temp_dir().join(format!(
            "offload-leftover-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().elapsed().map(|d| d.as_nanos())
        ));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "t@offload.local"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        std::fs::write(src.join("README.md"), "hi\n").expect("write");
        for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", "i"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        let source = RepoSource::Local(src.clone());

        let store = Store::open_memory().expect("store");
        let cfg = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        let sup = Supervisor::new(Arc::new(cfg), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();
        let peer = NodeId::from_bytes([2; 32]);

        // The checkout this node kept from the leg it ran before the run moved on.
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        // The record: the peer ran the surviving leg and finished it there.
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let epoch = run
            .assign(peer, now(), offload_core::LEASE)
            .expect("assign");
        run.started(peer, epoch, now()).expect("start");
        run.state = offload_core::RunState::Completed { at: now() };
        store.save_run(&run).expect("save");

        // …and this node's row for it, as gossip left it: the peer's position, under the peer's
        // name, at the peer's epoch. `live` is empty because this daemon has restarted since.
        let settled = offload_core::RunProgress {
            by: Some(peer),
            epoch,
            turns: 5,
            workspace: "3 modified".to_string(),
            at: Millis(1_000),
            ..Default::default()
        };
        store.save_stats(id, &settled).expect("stats");
        assert!(
            lock(&sup.live).get(&id).is_none(),
            "the case is a restarted daemon, so there must be no live entry"
        );

        assert!(
            matches!(
                sup.cleanup(id).await.expect("cleanup"),
                Removal::Removed { .. }
            ),
            "the checkout is really here and really goes — this is not session seventeen's case"
        );

        // The consequence, asked the way the fleet asks it: hand this node's row to the peer and
        // see whether it overwrites what the peer says about its own disk.
        let ours = store.load_stats(id).expect("stats");
        let mut theirs = settled.clone();
        theirs.absorb(&ours);
        assert_eq!(
            theirs.workspace, "3 modified",
            "`offload rm` here told the peer its own checkout was gone, while it sat there \
             untouched — which is session seventeen's bug reached by the other half of the door"
        );

        // The control, and the half that keeps the rule from being merely restrictive: the same
        // command on a run that ended *here*, whose row therefore carries this node's own stamp,
        // still says what it did. That is the case `writing_leg`'s `None` was written for, and it
        // is a restarted daemon too — the difference is whose name is on the row, which is the
        // whole distinction the fix turns on.
        let ours_to_say = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        sup.workspaces
            .hold(ours_to_say)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut here = Run::new(ours_to_say, spec(&source), sup.node_id, now());
        let epoch = here
            .assign(sup.node_id, now(), offload_core::LEASE)
            .expect("assign");
        here.started(sup.node_id, epoch, now()).expect("start");
        here.state = offload_core::RunState::Completed { at: now() };
        store.save_run(&here).expect("save");
        store
            .save_stats(
                ours_to_say,
                &offload_core::RunProgress {
                    by: Some(sup.node_id),
                    epoch,
                    turns: 5,
                    workspace: "3 modified".to_string(),
                    at: Millis(1_000),
                    ..Default::default()
                },
            )
            .expect("stats");
        assert!(matches!(
            sup.cleanup(ours_to_say).await.expect("cleanup"),
            Removal::Removed { .. }
        ));
        assert_eq!(
            store.load_stats(ours_to_say).expect("stats").workspace,
            "removed",
            "a node that owns the row must still be able to say what it did to its own disk"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A run in the store that no agent is attached to — what every run looks like to a
    /// daemon that has just restarted, and the only state `resume` will consider.
    fn dormant_run(sup: &Supervisor, store: &Store) -> (RunId, Run) {
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        (id, run)
    }

    /// A run in the store with a deadline, and no agent — a queued run as its arbiter sees it.
    fn queued_run(sup: &Supervisor, store: &Store, deadline: Option<Millis>) -> Run {
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut spec = spec(&RepoSource::parse("/tmp/whatever"));
        spec.queue = true;
        spec.deadline = deadline;
        let run = Run::new(id, spec, sup.node_id, now());
        store.save_run(&run).expect("save");
        run
    }

    fn overdue_events(store: &Store, run: RunId) -> Vec<LogEvent> {
        store
            .events_since::<LogEvent>(run, 0)
            .expect("events")
            .into_iter()
            .map(|(_, event)| event)
            .filter(|event| matches!(event.kind, LogKind::Overdue { .. }))
            .collect()
    }

    #[test]
    fn a_missed_deadline_is_announced_once_and_again_only_if_it_moves() {
        // The latch, and the one thing that clears it. An alarm that repeats every thirty
        // seconds for a night is one nobody reads; an alarm that never speaks again after the
        // operator gave the run another two hours has stopped describing anything true.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let mut run = queued_run(
            &sup,
            &store,
            Some(now().saturating_sub(Millis::from_mins(60))),
        );

        let waiting = || offload_core::Waiting::Placement { refused_by: 2 };
        assert!(sup.note_overdue(&run, Millis::from_mins(60), waiting()));
        assert!(!sup.note_overdue(&run, Millis::from_mins(61), waiting()));
        assert_eq!(overdue_events(&store, run.id).len(), 1);

        // `offload deadline <run> 2h`: a different stated time, so a different fact.
        run.spec.deadline = Some(now() + Millis::from_mins(120));
        assert!(sup.note_overdue(&run, Millis::from_mins(1), waiting()));
        assert_eq!(overdue_events(&store, run.id).len(), 2);
    }

    #[test]
    fn being_late_leaves_the_run_alone() {
        // The boundary from ADR-0013, as a test rather than a comment: a deadline decides when
        // we give up, never what happens to the work. Announcing one must not cancel it, fail
        // it, or move it out of the state its arbiter keeps offering it from.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let run = queued_run(
            &sup,
            &store,
            Some(now().saturating_sub(Millis::from_mins(5))),
        );

        sup.note_overdue(
            &run,
            Millis::from_mins(5),
            offload_core::Waiting::Placement { refused_by: 0 },
        );

        let after = store.load_run(run.id).expect("load").expect("run");
        assert_eq!(after.state, run.state);
        assert_eq!(after.epoch, run.epoch);
        assert!(!after.state.is_terminal());
    }

    #[test]
    fn only_the_node_holding_the_rule_may_retry_its_occurrence() {
        // ADR-0030's discriminator, over the two facts that already exist: `origin` travels and
        // `runs.rule` deliberately does not survive a gossip merge, so "tagged here" means "a rule
        // on this machine fired it". The wiring is the whole risk — the decision is tested in
        // `offload-core` — so this pins which of the three shapes gets which answer.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        // A run somebody submitted. Ours, whatever the tag says.
        let (_, operator) = dormant_run(&sup, &store);
        assert_eq!(
            sup.standing_to_retry(&operator),
            offload_core::Standing::Ours
        );

        // An occurrence with no local tag — what a peer's copy of somebody else's rule's
        // occurrence looks like, because a merge writes every column but that one.
        let (id, mut occurrence) = dormant_run(&sup, &store);
        occurrence = occurrence.started_by(offload_core::Origin::Rule);
        store.save_run(&occurrence).expect("save");
        assert_eq!(
            sup.standing_to_retry(&occurrence),
            offload_core::Standing::MachineStartedElsewhere,
            "an occurrence this node has no rule for is not this node's to retry"
        );

        // …and tagged, which is the rule's own node.
        let rule = offload_core::RuleId::from_bytes([7; 8]);
        store
            .add_rule(rule, "schedule", "{}", now().0 as i64)
            .expect("rule");
        assert!(store.tag_occurrence(id, rule).expect("tag"));
        assert_eq!(
            sup.standing_to_retry(&occurrence),
            offload_core::Standing::Ours
        );
    }

    /// The agent's own ceiling stops a start, and not only a bid.
    ///
    /// `WorkPolicy::admits` has this clause and `Room::for_one_more` does not, and `admits` is
    /// called by exactly one thing: the bid round. So the number `offload status` prints as
    /// *"claude-code sustains 2, which is what binds"* shaped what a node bid and stopped nothing.
    /// Measured on a fleet of one with **no cluster at all**, where `admits` is never reached:
    /// three submissions accepted without a word, `runs 3/3 · claude-code sustains 2, which is
    /// what binds`, and three agent processes for an install the node itself says sustains two.
    #[test]
    fn the_agents_own_ceiling_stops_a_start_and_not_only_a_bid() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (_, run) = dormant_run(&sup, &store);

        // Nothing has told this supervisor what the device is, which is every other test here:
        // the clause is skipped and the machine's own capacity is the only ceiling.
        assert!(sup.start_refusal(&run, Capacity::runs(4)).is_none());

        // Now it knows, and the install sustains none — the smallest statement of "full" there
        // is, and the one that needs no second run to set up.
        let mut caps = offload_core::Capabilities::empty(
            offload_core::capability::Os::Linux,
            offload_core::capability::Arch::X86_64,
            offload_core::capability::DeviceClass::Laptop,
        );
        caps.add(offload_core::Capability::agent(
            offload_core::AgentKind::ClaudeCode,
            offload_core::capability::AgentDetails {
                version: "9.9.9".into(),
                models: Default::default(),
                max_concurrent: 0,
            },
            true,
        ));
        sup.capabilities_via(std::sync::Arc::new(crate::deliver::Current::new(caps)));

        assert!(
            matches!(
                sup.start_refusal(&run, Capacity::runs(4)),
                Some(offload_core::Refusal::AgentAtCapacity(_))
            ),
            "the machine has room and the agent does not"
        );
    }

    #[test]
    fn a_held_run_can_say_why_it_has_not_started() {
        // What `offload explain` had no answer for. Its state line gave the lease's remaining
        // time and the canvass said "already holds this run", which is true and is not the
        // question — invisible while every reason was seconds long, and hours long once a rate
        // limit could be one of them.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);

        assert!(
            sup.start_refusal(&run, Capacity::runs(4)).is_none(),
            "nothing is holding it back, so it is about to start"
        );

        sup.judge_rate_limit(
            id,
            "five_hour",
            "rejected",
            Some((now() + Millis(60_000)).0),
        );
        let refusal = sup
            .start_refusal(&run, Capacity::runs(4))
            .expect("the limit is holding it");
        assert!(matches!(
            refusal,
            offload_core::Refusal::AccountRateLimited { .. }
        ));
        // The sentence an operator reads carries the fresh number…
        assert!(
            describe_refusal(&refusal).contains("for another"),
            "{}",
            describe_refusal(&refusal)
        );
        // …and the stored, gossiped one does not. Two renderings of one refusal, on purpose.
        assert_eq!(summarise_refusal(&refusal), "rate-limited");
    }

    #[test]
    fn every_held_run_summary_fits_the_column_it_is_printed_in() {
        // `offload ps`'s WORKSPACE column is 18 wide and its longest existing value —
        // `waiting for a slot` — is exactly that, so the column was sized to it. A longer string
        // pushes every following column off the end of the table, which is the misalignment
        // CLAUDE.md has an entry about, arrived at from the other direction: there the header and
        // the row disagreed, here the row outgrows both. Measured on a real daemon: the first
        // version of this said `account rate-limited`, which is twenty.
        //
        // The width is duplicated rather than shared, because `offload-cli` owns the table and
        // depends on this crate rather than the other way round. A test is the seam.
        for refusal in [
            offload_core::Refusal::AccountRateLimited { until: now() },
            offload_core::Refusal::AccountAtCapacity { running: 2, max: 2 },
            offload_core::Refusal::AgentAtCapacity(offload_core::AgentKind::ClaudeCode),
            offload_core::Refusal::AtCapacity { running: 4, max: 4 },
            offload_core::Refusal::BudgetFull {
                committed: 8,
                budget: 8,
                wanted: 4,
            },
            // The fall-through, which must also fit.
            offload_core::Refusal::NotAcceptingWork,
        ] {
            let summary = summarise_refusal(&refusal);
            check(&summary);
        }
        // The column's other tenant: what a held run's summary becomes once the run is over
        // without ever having started. Same column, same two rules.
        check(NEVER_STARTED);

        fn check(summary: &str) {
            const WORKSPACE_COLUMN: usize = 18;
            assert!(
                summary.chars().count() <= WORKSPACE_COLUMN,
                "{summary:?} is {} wide and the column is {WORKSPACE_COLUMN}",
                summary.chars().count()
            );
            // And no number in it, ever. This string is stored and gossiped, so a duration in it
            // is a sentence about a moment read back later as a claim about now — `offload rules`'
            // bug and `offload explain`'s, which is why the fresh number lives in those instead.
            assert!(
                !summary.chars().any(|c| c.is_ascii_digit()),
                "{summary:?} carries a number, and this string outlives the moment it was made"
            );
        }
    }

    #[test]
    fn a_rate_limit_with_a_reset_is_remembered_and_lifts_by_itself() {
        // ADR-0029. What was missing was not the signal — the agent emits one per turn and
        // `AgentEvent::RateLimit`'s own doc comment claims it "feeds per-account bid scoring" —
        // but anything at all that remembered it. So the next run was started into the same wall.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, _) = dormant_run(&sup, &store);
        assert_eq!(
            sup.account_limited_until(),
            None,
            "nothing has said otherwise"
        );

        // Informational, which the agent sends every turn. Not a limit.
        sup.judge_rate_limit(id, "five_hour", "allowed_warning", Some(now().0 + 60_000));
        assert_eq!(
            sup.account_limited_until(),
            None,
            "a quota report is not a hold-up — and this one arrives on every single turn"
        );

        // Blocking, with a stated reset.
        let lifts = now() + Millis::from_mins(30);
        sup.judge_rate_limit(id, "five_hour", "rejected", Some(lifts.0));
        assert_eq!(sup.account_limited_until(), Some(lifts));

        // The agent restates the same limit every turn, and a *later* reset for the same kind is
        // the news. An earlier one is the same fact told again: moving the value backwards would
        // let a run start into a limit still in force.
        sup.judge_rate_limit(id, "five_hour", "rejected", Some(lifts.0 - 60_000));
        assert_eq!(sup.account_limited_until(), Some(lifts));

        // Independent ceilings, and a run has to clear all of them, so the latest is the answer.
        let weekly = lifts + Millis::from_mins(60);
        sup.judge_rate_limit(id, "weekly", "rejected", Some(weekly.0));
        assert_eq!(sup.account_limited_until(), Some(weekly));

        // A block with **no** stated reset is not remembered at all: a fact with no expiry is
        // the thing that gets stuck, and the run that hit it is affected regardless.
        let fresh = supervisor_with(store.clone());
        fresh.judge_rate_limit(id, "five_hour", "rejected", None);
        assert_eq!(fresh.account_limited_until(), None);

        // And one that has lifted stops holding anything, without a tick having to remember to
        // clear it — the map is the whole state and an expired entry is not a fact.
        let past = supervisor_with(store);
        past.judge_rate_limit(id, "five_hour", "rejected", Some(now().0 - 1));
        assert_eq!(past.account_limited_until(), None);
    }

    #[test]
    fn a_rate_limit_is_judged_against_the_deadline_and_not_the_other_way_round() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        // Due in an hour, limit lifts in two: it has already been decided, an hour before the
        // deadline arrives, and nothing is gained by waiting to find out.
        let tight = queued_run(&sup, &store, Some(now() + Millis::from_mins(60)));
        sup.judge_rate_limit(
            tight.id,
            "five_hour",
            "rejected",
            Some((now() + Millis::from_mins(120)).0),
        );
        assert_eq!(overdue_events(&store, tight.id).len(), 1);

        // Due tomorrow: the wait fits, and a person told about this could do nothing with it.
        let roomy = queued_run(&sup, &store, Some(now() + Millis::from_mins(1_440)));
        sup.judge_rate_limit(
            roomy.id,
            "five_hour",
            "rejected",
            Some((now() + Millis::from_mins(120)).0),
        );
        assert!(overdue_events(&store, roomy.id).is_empty());
    }

    #[test]
    fn a_quota_report_is_not_a_delay_and_no_deadline_is_not_a_missed_one() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        // The agent sends one of these per turn. This run is an hour past due and the account
        // is fine, so there is nothing to say — without the status check, every overdue run in
        // the fleet announces itself once a turn.
        let late = queued_run(
            &sup,
            &store,
            Some(now().saturating_sub(Millis::from_mins(60))),
        );
        sup.judge_rate_limit(late.id, "five_hour", "allowed", Some(now().0));
        sup.judge_rate_limit(late.id, "five_hour", "allowed_warning", Some(now().0));
        assert!(overdue_events(&store, late.id).is_empty());

        // And the trap's fourth appearance: no stated deadline means the moment of submission,
        // so this run is overdue on every other measure here and has missed nothing.
        let asap = queued_run(&sup, &store, None);
        sup.judge_rate_limit(
            asap.id,
            "five_hour",
            "rejected",
            Some((now() + Millis::from_mins(600)).0),
        );
        assert!(overdue_events(&store, asap.id).is_empty());
    }

    #[tokio::test]
    async fn a_checkpoint_is_refused_while_the_transcript_holds_no_conversation() {
        // Measured on a real agent, twice, at the same turn 1: one capture held 22,953 bytes
        // with the conversation in it and one held 17,087 bytes with none of it — the agent
        // writes its transcript behind its own event stream by an amount it does not promise,
        // so this is a race and not a fixed lag.
        //
        // What the empty one causes is the worst-shaped failure available. Walked: the run was
        // resumed onto a rebuilt worktree, the stored transcript was installed over the agent's
        // own (`transcript restored from checkpoint`), and the agent — having lost its half of
        // the conversation — replied "No response requested.", made no tool call, wrote nothing,
        // and the run reported **`Completed`**. An abandoned run that reads as a success. The
        // control, whose capture had caught up, resumed and wrote the nanosecond timestamp that
        // existed only in the conversation.
        //
        // So this is the rule the file already stated one block up — better to fail the
        // checkpoint loudly than record one that cannot do its job — applied to the case the
        // existing check missed, because it asked whether the *file* was there rather than
        // whether there was a conversation in it.
        let dir = std::env::temp_dir().join(format!("offload-emptytx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        let store = Store::open_memory().expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: dir.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(dir.join("home"));

        let run_id = RunId::from_bytes([43; 16]);
        let workspace = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        let session = "11111111-2222-3333-4444-555555555555";
        // Everything a real lagging transcript has *except* the assistant rows — the row types
        // measured in the 17,087-byte capture. Well-formed, substantial, and useless to resume
        // from, which is the whole difficulty: nothing about it looks wrong.
        let lagging = b"{\"type\":\"queue-operation\"}\n{\"type\":\"user\",\"message\":{\"content\":\"do the thing\"}}\n{\"type\":\"attachment\"}\n{\"type\":\"ai-title\"}\n";
        transcript::install(&sup.home, &workspace.path, session, lagging).expect("transcript");

        let refused = sup
            .checkpoint(run_id, &workspace, session, "2.1.251", 1, None)
            .await
            .expect_err("a capture with no conversation in it must not be recorded");
        assert!(
            refused.to_string().contains("no conversation"),
            "and it has to say which of the several things that can fail here did: {refused}"
        );
        assert!(
            store
                .load_run(run_id)
                .expect("load")
                .expect("present")
                .checkpoint
                .is_none(),
            "nothing may be recorded, or a later resume finds a checkpoint that cannot resume"
        );

        // And the moment the agent's writes land, the same call succeeds — this is a wait, not
        // a broken run. The caller treats the refusal as `CaptureFailed` and tries again at the
        // next boundary, by which time this is what the file looks like.
        transcript::install(&sup.home, &workspace.path, session, &conversation(1))
            .expect("transcript");
        sup.checkpoint(run_id, &workspace, session, "2.1.251", 1, None)
            .await
            .expect("the capture succeeds once the transcript has caught up");
        assert!(store
            .load_run(run_id)
            .expect("load")
            .expect("present")
            .checkpoint
            .is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A transcript with a *conversation* in it, which is what `checkpoint` now requires.
    ///
    /// The fixtures here used to be `{"turn":1}` — well-formed JSONL that no agent would ever
    /// write and that carries no conversation at all. That passed for as long as nothing looked
    /// inside, and the thing that started looking found a real failure with it: a capture whose
    /// transcript has no assistant messages resumes into an agent that has lost its own half of
    /// the conversation. A fixture that could not have been produced by the thing it stands in
    /// for is a fixture that tests the wrong system.
    fn conversation(turns: u32) -> Vec<u8> {
        let mut out = String::new();
        for turn in 1..=turns.max(1) {
            out.push_str(&format!(
                "{{\"type\":\"assistant\",\"message\":{{\"id\":\"msg_{turn}\",\"content\":[{{\"type\":\"text\",\"text\":\"turn {turn}\"}}],\"usage\":{{\"input_tokens\":5,\"output_tokens\":7}}}}}}\n"
            ));
        }
        out.into_bytes()
    }

    /// A checkpoint whose transcript is actually **on this node** — which is what every run
    /// the product resumes has, and what `checkpoint_at` on its own does not.
    ///
    /// Three tests resumed from a checkpoint naming a blob that existed nowhere and passed,
    /// which is the defect written down: `resume` reached `launch` without fetching anything,
    /// and the run died inside the restore. They take this now, so that a test which really
    /// is about a *missing* blob has to say so.
    fn checkpoint_here(store: &Store, turn: u32) -> Checkpoint {
        Checkpoint {
            transcript: store
                .put_blob(format!("the conversation at turn {turn}").as_bytes())
                .expect("blob"),
            ..checkpoint_at(turn)
        }
    }

    fn checkpoint_at(turn: u32) -> Checkpoint {
        Checkpoint {
            session_id: Some("11111111-2222-3333-4444-555555555555".into()),
            transcript: offload_core::BlobHash::from_bytes([3; 32]),
            bundle: None,
            patch: None,
            base_commit: "0f0f0f0f".into(),
            turns: turn,
            taken_at: now(),
            agent_version: "2.1.220".into(),
            replicas: std::collections::BTreeSet::new(),
        }
    }

    /// A git repo with one commit, for the tests that need real git rather than a mock —
    /// worktree and bundle semantics are exactly what a mock would get wrong.
    async fn git_repo(at: &std::path::Path) -> RepoSource {
        std::fs::create_dir_all(at).expect("mkdir");
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "t@offload.local"],
            vec!["config", "user.name", "Offload Test"],
        ] {
            run_git(at, &args).await;
        }
        std::fs::write(at.join("README.md"), "hello\n").expect("write");
        commit(at, "start.rs", "fn start() {}\n").await;
        RepoSource::Local(at.to_path_buf())
    }

    async fn commit(at: &std::path::Path, file: &str, contents: &str) {
        std::fs::write(at.join(file), contents).expect("write");
        run_git(at, &["config", "user.email", "t@offload.local"]).await;
        run_git(at, &["config", "user.name", "Offload Test"]).await;
        run_git(at, &["add", "."]).await;
        run_git(at, &["commit", "--quiet", "-m", "work"]).await;
    }

    async fn run_git(at: &std::path::Path, args: &[&str]) {
        let status = tokio::process::Command::new("git")
            .arg("-C")
            .arg(at)
            .args(args)
            .status()
            .await
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// What a node gossips as "what I am running" is what it *holds*.
    ///
    /// The store keeps records this node never ran: runs it submitted and placed elsewhere, and —
    /// since ADR-0025 — work it neither ran nor submitted, because a bystander's record is the
    /// index `offload logs` resolves an id in. Unfiltered, every node that had heard of a run
    /// claimed to be running it. Measured on two daemons: one run held by alpha, and `offload
    /// nodes` printed `RUNS 1` on both rows on both machines, about a node whose own status said
    /// `runs 0/2` and which had not been granted `host-runs` at all.
    #[test]
    fn a_node_gossips_the_runs_it_holds_and_not_the_ones_it_has_heard_of() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let somebody_else = NodeId::from_bytes([9; 32]);

        // Ours: assigned here, which is what a holder is — `Assigned` included, because a
        // commitment is a run this node is answerable for (ADR-0006).
        let mut mine = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        mine.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&mine).expect("save");

        // A stub learned by gossip: a peer's run, live, that this node has nothing to do with.
        let mut theirs = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            somebody_else,
            now(),
        );
        let epoch = theirs
            .assign(somebody_else, now(), LEASE_TTL)
            .expect("assign");
        theirs
            .started(somebody_else, epoch, now())
            .expect("their agent, on their machine");
        store.save_run(&theirs).expect("save");

        let gossiped = sup.active_runs();
        assert!(gossiped.contains(&mine.id), "we are holding that one");
        assert!(
            !gossiped.contains(&theirs.id),
            "and merely knowing about a run is not running it"
        );
        assert_eq!(gossiped.len(), 1);
    }

    /// A scratch git repo and the directory holding it, for a test that needs a real checkout.
    ///
    /// The four tests that predate it build this inline; this is not a refactor of them —
    /// two of them depend on the exact shape of what they commit (a 16 MB file, to widen a
    /// window) and folding those into one helper would hide the thing being measured.
    fn scratch_repo(tag: &str) -> (PathBuf, RepoSource) {
        let dir = std::env::temp_dir().join(format!(
            "offload-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().elapsed().map(|d| d.as_nanos())
        ));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "t@offload.local"],
            vec!["config", "user.name", "t"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        std::fs::write(src.join("README.md"), "hi\n").expect("write");
        for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", "i"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(&src)
                .status()
                .expect("git");
        }
        (dir, RepoSource::Local(src))
    }

    fn spec(source: &RepoSource) -> RunSpec {
        RunSpec {
            work: Work::Agent(AgentWork {
                agent: AgentKind::ClaudeCode,
                model: None,
                prompt: "do a thing".into(),
                workspace: WorkspaceSpec {
                    repo: source.clone_target(),
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
        }
    }

    fn submission() -> crate::api::SubmitRequest {
        crate::api::SubmitRequest {
            max_turns: None,
            repo: "/tmp/whatever".into(),
            prompt: "do a thing".into(),
            model: None,
            permission: None,
            git_ref: None,
            allow: Vec::new(),
            queue: false,
            deadline: None,
            demand: Default::default(),
            notify: Default::default(),
            notices: Default::default(),
            ask: offload_core::AskPolicy::Never,
            resources: Vec::new(),
            origin: offload_core::Origin::Operator,
            require: offload_core::Wanted::default(),
            prefer: offload_core::Wanted::default(),
            hold_until: None,
        }
    }

    /// A run held here, at its own epoch, that may stop and ask.
    fn asking_run(sup: &Supervisor, store: &Store, ask: offload_core::AskPolicy) -> Run {
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut spec = spec(&RepoSource::parse("/tmp/whatever"));
        spec.agent_mut().expect("an agent run").ask = ask;
        let mut run = Run::new(id, spec, sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        run
    }

    #[test]
    fn what_is_worth_asking_about_depends_on_what_the_agent_is_gating() {
        // The reason ADR-0008's refusal of `Ask` stood as long as it did. Under the product
        // default the agent allows edits by itself, so matching `Write` would stop a run to ask
        // about something nobody was ever going to be asked (ADR-0004's line, the noisy side).
        for mode in [PermissionMode::AcceptEdits, PermissionMode::Full] {
            let tools = ask_tools(mode);
            assert!(tools.contains(&"Bash"), "{mode:?} must cover commands");
            assert!(
                !tools.contains(&"Write"),
                "{mode:?} lets the agent edit, so asking about it is noise"
            );
        }
        // Under `Ask` the agent gates every edit, and the *silent* side of the same line is
        // worse: a run that can be asked about a command and is quietly denied every file.
        let manual = ask_tools(PermissionMode::Ask);
        for tool in ["Bash", "WebFetch", "Write", "Edit", "NotebookEdit"] {
            assert!(manual.contains(&tool), "`ask` mode must cover {tool}");
        }
        // Measured, not assumed: the agent allows reads under `manual`, so a matcher buys
        // questions and nothing else.
        assert!(!manual.contains(&"Read"));
    }

    #[tokio::test]
    async fn ask_mode_is_accepted_once_something_can_answer_for_it() {
        // The other half of the refusal below, and the whole point of widening the tool list:
        // `--permission ask --ask` is a run every gate of which reaches a person.
        let sup = supervisor();
        sup.submit(
            crate::api::SubmitRequest {
                permission: Some(PermissionMode::Ask),
                ask: offload_core::AskPolicy::UpTo { questions: 5 },
                resources: Vec::new(),
                ..submission()
            },
            Capacity::runs(4),
        )
        .await
        .expect("ask mode with somewhere to ask is answerable");
    }

    #[tokio::test]
    async fn a_run_stops_asking_once_its_budget_is_spent_and_says_so_once() {
        // The question ADR-0017 left open, answered. Widening to edits means an ordinary coding
        // run would put thirty questions to somebody's phone, and the thirtieth is not read.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let run = asking_run(&sup, &store, offload_core::AskPolicy::UpTo { questions: 2 });

        for n in 0..2 {
            sup.ask(
                run.id,
                run.epoch,
                &format!("toolu_{n}"),
                "Bash",
                "rm -rf /",
                true,
            )
            .unwrap_or_else(|e| panic!("question {n} should be asked: {e}"));
        }
        assert_eq!(
            store.load_stats(run.id).expect("stats").asks,
            2,
            "spent when the question is put, so the budget means what it says"
        );

        let err = sup
            .ask(run.id, run.epoch, "toolu_2", "Bash", "rm -rf /", true)
            .expect_err("the third question is not asked");
        assert!(matches!(err, NoAsk::BudgetSpent { asked: 2 }));
        // Not a denial, and the wording has to say so: what happens to this call is the agent's
        // own rules, exactly as if the run had never been able to ask.
        assert!(err.to_string().contains("agent's own rules"));

        // Said once. A second refusal must not log a second sentence — an alarm that repeats
        // every tool call for the rest of the night is an alarm people filter.
        let _ = sup.ask(run.id, run.epoch, "toolu_3", "Bash", "rm -rf /", true);
        let spent: Vec<_> = store
            .events_since::<LogEvent>(run.id, 0)
            .expect("events")
            .into_iter()
            .filter(|(_, e)| matches!(e.kind, LogKind::AskBudgetSpent { .. }))
            .collect();
        assert_eq!(spent.len(), 1, "said once per run, not once per tool call");
    }

    #[tokio::test]
    async fn a_question_nobody_was_asked_costs_nothing() {
        // The budget bounds *interruptions*. A call an existing grant already covers interrupted
        // nobody, so charging the run for it would spend somebody's attention on a question they
        // were never shown — and then stop asking the ones that matter.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let mut run = asking_run(&sup, &store, offload_core::AskPolicy::UpTo { questions: 1 });
        run.spec.agent_mut().expect("an agent run").allow =
            ToolAllowlist::parse(["Bash(cargo test:*)"]).expect("parse");
        store.save_run(&run).expect("save");

        let err = sup
            .ask(
                run.id,
                run.epoch,
                "toolu_0",
                "Bash",
                "cargo test --all",
                true,
            )
            .expect_err("a granted call is not a question");
        assert!(matches!(err, NoAsk::AlreadyGranted));
        assert_eq!(store.load_stats(run.id).expect("stats").asks, 0);

        // And the budget is still there for the call that does need it.
        sup.ask(run.id, run.epoch, "toolu_1", "Bash", "rm -rf /", true)
            .expect("the ungranted one is asked");
        assert_eq!(store.load_stats(run.id).expect("stats").asks, 1);
    }

    #[tokio::test]
    async fn ask_is_refused_at_submit_not_discovered_later() {
        // ADR-0008. Accepting a configuration we know cannot work and letting it fail
        // forty turns in is worse than refusing it immediately.
        let sup = supervisor();
        let err = sup
            .submit(
                crate::api::SubmitRequest {
                    permission: Some(PermissionMode::Ask),
                    ..submission()
                },
                Capacity::runs(4),
            )
            .await
            .expect_err("ask must be refused");

        assert!(matches!(err, SubmitError::AskIsUnanswerable));
        let message = err.to_string();
        assert!(message.contains("accept-edits"), "must name the way out");
        assert!(message.contains("0008"), "must point at the reasoning");
    }

    #[test]
    fn a_granted_resource_travels_with_the_run_and_does_not_pin_it() {
        // This was the opposite assertion until the proxied call existed, and the ADR's own
        // justification is what flipped it: a grant constrained placement only "until a resource
        // can be reached across the mesh". Keeping the constraint would have defeated the case
        // ADR-0011 was written for — the phone holds the mailbox and hosts nothing, so
        // `--use email` would pin the run to a device that cannot run it.
        let sup = supervisor();
        let run = sup
            .build(
                crate::api::SubmitRequest {
                    resources: vec![offload_core::Service::Email],
                    ..submission()
                },
                &sup.members_alone(),
            )
            .expect("build");

        // The grant is on the spec, so it travels and a migrated run is re-projected against
        // whatever the new holder can reach.
        assert_eq!(run.spec.resources, vec![offload_core::Service::Email]);

        // And a node with no mailbox of its own satisfies the run's constraint, because the
        // call is forwarded rather than the run moved.
        // A node with an authenticated agent and nothing else: no mailbox, no resources.
        let mut caps = offload_core::Capabilities::empty(
            offload_core::Os::Linux,
            offload_core::Arch::X86_64,
            offload_core::DeviceClass::Laptop,
        );
        let mut agent = offload_core::Capability::new(
            "agent:claude-code",
            offload_core::Service::Agent(AgentKind::ClaudeCode),
            [offload_core::Role::Execute],
        );
        agent.authenticated = true;
        agent.details = offload_core::ServiceDetails::Agent(offload_core::AgentDetails {
            version: "2.1.238".into(),
            models: Vec::new(),
            max_concurrent: 2,
        });
        caps.add(agent);
        let explained = run.spec.constraint.explain(&caps);
        assert!(
            explained.failures().is_empty(),
            "a granted run must be placeable on a node that does not hold the resource: {:?}",
            explained.failures()
        );

        // And a run that asked for nothing is constrained exactly as it always was.
        let plain = sup
            .build(submission(), &sup.members_alone())
            .expect("build");
        assert!(plain.spec.resources.is_empty());
        assert_eq!(plain.spec.constraint, run.spec.constraint);
    }

    #[tokio::test]
    async fn capacity_is_enforced_before_any_work_starts() {
        let sup = supervisor();
        let err = sup
            .submit(submission(), Capacity::runs(0))
            .await
            .expect_err("zero capacity means no runs");
        assert!(matches!(
            err,
            SubmitError::NoRoom(offload_core::Refusal::AtCapacity { max: 0, .. })
        ));
    }

    #[tokio::test]
    async fn a_submitted_run_is_durable_immediately() {
        // Phase 1 kept this in memory, so a crash between submit and finish lost the run
        // entirely — including the fact that a worktree had been created for it.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let stored = store.load_run(id).expect("load").expect("present");
        assert_eq!(
            stored.spec.agent().expect("an agent run").prompt,
            "do a thing"
        );
        assert_eq!(
            stored.epoch.0, 1,
            "assigned to this node, so the epoch has advanced"
        );
    }

    #[tokio::test]
    async fn recovery_fails_runs_whose_daemon_is_gone() {
        // An agent process dies with the daemon that spawned it. Leaving the row as
        // `running` would make `ps` claim work is happening that is not — worse than an
        // honest failure, because the user waits for output that will never come.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let restarted = supervisor_with(store.clone());
        let recovered = restarted.recover().expect("recover");
        assert_eq!(recovered, 1);

        let run = store.load_run(id).expect("load").expect("present");
        assert!(run.state.is_terminal(), "no longer claims to be running");
        assert!(
            failure_reason(&run).is_some_and(|r| r.contains("restarted")),
            "and says why"
        );
    }

    #[tokio::test]
    async fn recovery_leaves_finished_runs_alone() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let mut run = store.load_run(id).expect("load").expect("present");
        run.complete(NodeId::from_bytes([1; 32]), run.epoch, now())
            .expect("complete");
        store.save_run(&run).expect("save");

        assert_eq!(
            supervisor_with(store.clone()).recover().expect("recover"),
            0,
            "a completed run is not an interrupted one"
        );
    }

    #[tokio::test]
    async fn logs_replay_from_the_store_after_a_restart() {
        // The concrete thing phase 1 could not do: `offload logs` on a run the current
        // daemon never hosted.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let restarted = supervisor_with(store);
        let (backlog, live) = restarted.subscribe(id).expect("subscribe");

        assert!(!backlog.is_empty(), "the submitted event replays");
        assert!(
            live.is_none(),
            "and there is no live channel to wait on, so --follow returns rather than hangs"
        );
    }

    #[tokio::test]
    async fn a_departing_node_pushes_the_last_copy_and_hands_the_run_over_instead_of_stranding_it()
    {
        // **ADR-0054, and the run in the test above that had to be left behind.** `fleet_could_
        // start` decides between handing a failed run to the fleet and stranding it with
        // `NoCopyElsewhere`, by asking `is_durable` — and until this pass existed nothing on the
        // way out had ever tried to change that answer. A checkpoint taken while no peer was
        // reachable stays `here only` for ever (`replicate` has one caller and no retry), and a
        // failed run has no next capture, so "there was no peer then" and "there is one now" were
        // both true at the moment the node left and nobody joined them up.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let peer = NodeId::from_bytes([9; 32]);
        let asked = Arc::new(Mutex::new(Vec::new()));
        sup.peers_via(Arc::new(FakeReplicator {
            taker: Some(peer),
            asked: asked.clone(),
        }));
        let (alone, mut solo) = dormant_run(&sup, &store);
        solo.checkpoint = Some(checkpoint_at(4));
        store.save_run(&solo).expect("save");
        let _cancel = sup.register(alone, solo.epoch).expect("registered");
        let solo_epoch = solo.epoch;
        sup.fail(alone, "agent reported failure".to_string());
        sup.stop_accepting();

        assert!(
            !store
                .load_run(alone)
                .expect("load")
                .expect("present")
                .checkpoint
                .expect("checkpoint")
                .is_durable(None),
            "the staging: one copy, on this node"
        );

        let copies = sup
            .push_last_copies(std::time::Duration::from_secs(30))
            .await;
        assert_eq!(copies.pushed, 1, "the copy that did not exist a moment ago");
        assert_eq!(copies.refused + copies.unattempted, 0);
        assert!(copies.worth_saying(), "and the drain has something to say");

        // `false`: this test has a peer, and has just pushed it a copy of the checkpoint.
        sup.hand_back_failed_runs(false).await;

        let handed = store.load_run(alone).expect("load").expect("present");
        assert_eq!(
            handed.let_go_by(),
            Some(sup.node_id),
            "handed to the fleet, where it used to be left for a person"
        );
        assert!(
            handed.epoch > solo_epoch,
            "the leg that failed cannot act on it"
        );
        assert_ne!(
            lock(&sup.recovery).get(&alone).and_then(|r| r.decided),
            Some(offload_core::Escalation::NoCopyElsewhere),
            "and not for the reason this pass exists to remove"
        );
    }

    #[tokio::test]
    async fn the_last_copy_pass_skips_what_needs_nothing_and_is_bounded_when_nobody_answers() {
        // Three properties in one staging, because they are three arms of the same loop and a
        // test per arm would restate the setup three times.
        //
        // **Already held** is the ordinary drain: nearly every checkpoint has a replica because
        // `checkpoint` made one at the time, and a pass that re-pushed them all would spend a
        // departing node's last seconds on bytes the fleet already has. **A run that stopped by
        // decision** is the collector's rule met here: nothing can start from a `Completed`
        // capture, so copying it is bytes spent on work nobody will do. And **the budget** is
        // what stops a slow peer turning a departure into a hang — the honest failure is a run
        // unattempted with a line saying so.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        sup.stop_accepting();

        // Already somewhere else.
        let peer = NodeId::from_bytes([9; 32]);
        let mut replicated = checkpoint_at(4);
        replicated.replicas.insert(peer);
        let (_safe, mut safe_run) = dormant_run(&sup, &store);
        safe_run.checkpoint = Some(replicated);
        store.save_run(&safe_run).expect("save");

        // Stopped on purpose, so its capture is a note about the past.
        let (done, mut done_run) = dormant_run(&sup, &store);
        done_run.checkpoint = Some(checkpoint_at(4));
        done_run
            .complete(sup.node_id, done_run.epoch, now())
            .expect("complete");
        store.save_run(&done_run).expect("save");
        assert!(
            store
                .load_run(done)
                .expect("load")
                .expect("present")
                .resumable_checkpoint()
                .is_none(),
            "the staging: a completed run has nothing to start from"
        );

        // And one that really does need a copy, against a fleet that never answers.
        let (_risky, mut risky) = dormant_run(&sup, &store);
        risky.checkpoint = Some(checkpoint_at(4));
        store.save_run(&risky).expect("save");

        sup.peers_via(Arc::new(NeverAnswers));

        let copies = sup
            .push_last_copies(std::time::Duration::from_millis(100))
            .await;

        assert_eq!(copies.already, 1, "the replicated one was left alone");
        assert_eq!(
            copies.pushed + copies.refused,
            0,
            "nothing was copied to a fleet that never answered"
        );
        assert_eq!(
            copies.unattempted, 1,
            "and the one at risk is reported as unattempted rather than as refused: the \
             difference is whether the fleet was willing, and they need opposite fixes"
        );
    }

    /// A fleet that never answers a request to take a copy.
    ///
    /// `GatedFetch` gates `fetch` and answers `replicate` instantly, which is the opposite end of
    /// the same trait — a test using it here measures a *refusal*, not a timeout, and the two are
    /// the distinction `LastCopies` exists to draw.
    #[derive(Debug)]
    struct NeverAnswers;

    #[async_trait::async_trait]
    impl Peers for NeverAnswers {
        async fn replicate(&self, _run: RunId, _blobs: Vec<offload_core::BlobHash>) -> Replicated {
            std::future::pending().await
        }

        async fn fetch(&self, _blob: offload_core::BlobHash) -> Result<(), String> {
            Err("this test's fleet has no blobs".into())
        }
    }

    /// A replicator that records what it was asked to copy and answers however the test wants.
    #[derive(Debug)]
    struct FakeReplicator {
        taker: Option<NodeId>,
        asked: Arc<Mutex<Vec<offload_core::BlobHash>>>,
    }

    #[async_trait::async_trait]
    impl Peers for FakeReplicator {
        async fn replicate(&self, _run: RunId, blobs: Vec<offload_core::BlobHash>) -> Replicated {
            lock(&self.asked).extend(blobs);
            match self.taker {
                Some(peer) => Replicated::To(peer),
                None => Replicated::NoPeer,
            }
        }

        async fn fetch(&self, _blob: offload_core::BlobHash) -> Result<(), String> {
            Err("this test's fleet has no blobs".into())
        }
    }

    /// Wait for the spawned replication task to finish, without sleeping on a guess.
    async fn settle() {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn a_checkpoint_is_copied_off_this_machine_and_the_run_records_where() {
        // ADR-0016: a checkpoint that exists only on the node that died cannot be migrated
        // from. This is the part that makes the run's own record say whether it survived.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let peer = NodeId::from_bytes([9; 32]);
        let asked = Arc::new(Mutex::new(Vec::new()));
        sup.peers_via(Arc::new(FakeReplicator {
            taker: Some(peer),
            asked: asked.clone(),
        }));

        let (id, mut run) = dormant_run(&sup, &store);
        // **All three, because the assertion below claims all three.** With `checkpoint_at`'s
        // bare transcript this read `[transcript] == [transcript]` and could not fail: measured
        // by making `replicate` send `vec![checkpoint.transcript]`, which passed the whole
        // workspace — 909 tests — with the one invariant `mesh.rs` calls "all or nothing"
        // silently gone. A peer holding the conversation and not the commits is recorded as a
        // replica, and `is_durable` then says a run is safe that nobody else can rebuild.
        let checkpoint = Checkpoint {
            bundle: Some(offload_core::BlobHash::from_bytes([7; 32])),
            patch: Some(offload_core::BlobHash::from_bytes([8; 32])),
            ..checkpoint_at(3)
        };
        let blobs = checkpoint.blobs();
        assert_eq!(blobs.len(), 3, "the staging: a capture with commits in it");
        run.record_checkpoint(sup.node_id, run.epoch, checkpoint.clone())
            .expect("record");
        store.save_run(&run).expect("save");

        sup.replicate(id, checkpoint);
        settle().await;

        assert_eq!(lock(&asked).clone(), blobs, "every blob, not some of them");
        let stored = store.load_run(id).expect("load").expect("present");
        let checkpoint = stored.checkpoint.expect("checkpoint");
        assert!(checkpoint.replicas.contains(&peer));
        assert!(checkpoint.is_durable(Some(sup.node_id)));
    }

    #[tokio::test]
    async fn a_run_a_drain_released_is_still_here_and_held_count_cannot_see_it() {
        // The premise `offload drain` reported from, and the reason it printed `nothing to hand
        // over` about a run it had just stopped. `held_count` answers a *capacity* question —
        // how many runs is this node holding — and a checkpoint captured with `release = true`
        // hands the run back to the pool, so it has no holder by design. The record is still
        // here, still unfinished, and its agent has been killed; the number that is supposed to
        // say "still here" counted none of that.
        //
        // Pinned here rather than at the drain, because `Mesh::drain` needs a QUIC transport and
        // a peer willing to refuse. What guards the fix is that the count is taken inside the
        // pass, and what guards the reader is this: anybody reaching for `held_count()` as "how
        // many runs are on this machine" finds it contradicted.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let (id, mut run) = dormant_run(&sup, &store);
        assert_eq!(sup.held_count(), 1, "running here");

        run.checkpointed(
            sup.node_id,
            run.epoch,
            checkpoint_at(7),
            GivenUp::Parked,
            now(),
        )
        .expect("checkpoint and release");
        store.save_run(&run).expect("save");

        assert_eq!(sup.held_count(), 0, "released, so not held — by design");
        let listed = sup
            .list(&|id| id.short())
            .into_iter()
            .find(|r| r.id == id.to_string())
            .expect("and still perfectly present");
        assert_eq!(listed.state, "pending");
    }

    #[tokio::test]
    async fn ps_keeps_the_here_only_warning_for_the_run_that_can_still_lose_its_work() {
        // The `SAFE` column, and the note under the table that says what it means: "that run's
        // checkpoint exists on one machine. If it goes away, so does the work." It was suppressed
        // for every terminal run — and `Failed` is terminal and *reopens*, so the one row the
        // warning was written for is the one row that never showed it. Three states, because the
        // point is the distinction and not the fix: a decision to stop still says nothing.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut safety = std::collections::BTreeMap::new();
        for state in ["failed", "completed", "cancelled"] {
            let (id, mut run) = dormant_run(&sup, &store);
            run.record_checkpoint(sup.node_id, run.epoch, checkpoint_at(6))
                .expect("record");
            match state {
                "failed" => run
                    .fail(sup.node_id, run.epoch, "the agent died", now())
                    .expect("fail"),
                "completed" => run.complete(sup.node_id, run.epoch, now()).expect("done"),
                _ => run.cancel(now()).expect("cancel"),
            }
            store.save_run(&run).expect("save");
            let listed = sup
                .list(&|id| id.short())
                .into_iter()
                .find(|r| r.id == id.to_string())
                .expect("listed");
            safety.insert(state, listed.checkpoint_durable);
        }

        assert_eq!(
            safety.get("failed"),
            Some(&Some(false)),
            "a failed run's checkpoint on one machine only is the alarm ADR-0016 exists to raise"
        );
        assert_eq!(safety.get("completed"), Some(&None), "nothing left to lose");
        assert_eq!(safety.get("cancelled"), Some(&None), "nor here");
    }

    #[tokio::test]
    async fn a_run_nobody_will_take_a_copy_of_stays_marked_as_only_here() {
        // A fleet of one, or a fleet that declined. Either way the run is not durable, and
        // saying so is the whole point — a checkpoint quietly believed safe is worse than one
        // known to be fragile.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        sup.peers_via(Arc::new(FakeReplicator {
            taker: None,
            asked: Arc::new(Mutex::new(Vec::new())),
        }));

        let (id, mut run) = dormant_run(&sup, &store);
        let checkpoint = checkpoint_at(1);
        run.record_checkpoint(sup.node_id, run.epoch, checkpoint.clone())
            .expect("record");
        store.save_run(&run).expect("save");

        sup.replicate(id, checkpoint);
        settle().await;

        let stored = store.load_run(id).expect("load").expect("present");
        let checkpoint = stored.checkpoint.expect("checkpoint");
        assert!(checkpoint.replicas.is_empty());
        assert!(!checkpoint.is_durable(Some(sup.node_id)));
    }

    #[tokio::test]
    async fn a_replica_confirmed_late_does_not_overwrite_a_newer_checkpoint() {
        // Turns keep happening while bytes are in flight. Writing back the checkpoint the
        // replication started with would roll the run back a turn — silently, and only under
        // load, which is the worst way to find a bug.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        sup.peers_via(Arc::new(FakeReplicator {
            taker: Some(NodeId::from_bytes([9; 32])),
            asked: Arc::new(Mutex::new(Vec::new())),
        }));

        let (id, mut run) = dormant_run(&sup, &store);
        let turn_one = checkpoint_at(1);
        run.record_checkpoint(sup.node_id, run.epoch, turn_one.clone())
            .expect("record");
        store.save_run(&run).expect("save");

        // Turn 2 lands while turn 1's bytes are still in flight — which is the ordinary case
        // at one checkpoint per turn, not a rare one.
        let mut newer = store.load_run(id).expect("load").expect("present");
        let mut turn_two = checkpoint_at(2);
        turn_two.taken_at = turn_one.taken_at + Millis(1);
        newer
            .record_checkpoint(sup.node_id, newer.epoch, turn_two)
            .expect("record");
        store.save_run(&newer).expect("save");

        sup.replicate(id, turn_one);
        settle().await;

        let stored = store.load_run(id).expect("load").expect("present");
        let checkpoint = stored.checkpoint.expect("checkpoint");
        assert_eq!(checkpoint.turns, 2, "the newer checkpoint survived");
        assert!(
            checkpoint.replicas.is_empty(),
            "and was not credited with a copy that was made of the older one"
        );
    }

    #[tokio::test]
    async fn a_migrated_transcript_is_installed_where_this_node_will_look_for_it() {
        // The bug this exists to prevent, found by migrating a real run: two daemons sharing
        // a home directory each keep transcripts under a slug derived from *their* worktree
        // path. Deciding "is one already here?" by scanning every project directory finds the
        // other node's copy, skips the install, and the agent then starts a fresh
        // conversation while reporting that it resumed one.
        let scratch = std::env::temp_dir().join(format!("offload-slug-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        let home = scratch.join("home");
        let session = "cc0c4f0b-095d-4d9f-8217-dc239d26482b";

        // Node A's copy, under node A's worktree path.
        let theirs = offload_agent::transcript::install(
            &home,
            &scratch.join("a/worktrees/run"),
            session,
            b"{\"type\":\"user\"}\n",
        )
        .expect("install");
        assert!(theirs.is_file());

        // From node B's worktree, a scan still finds it — which is exactly the trap.
        let mine = scratch.join("b/worktrees/run");
        assert!(offload_agent::transcript::find(&home, &mine, session).is_ok());
        assert!(
            !offload_agent::transcript::expected_path(&home, &mine, session).is_file(),
            "and the path this node's agent will actually use is empty"
        );
        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn a_restart_does_not_fail_a_run_another_node_is_happily_running() {
        // With a mesh this store holds runs placed elsewhere. Failing one of those because
        // *we* restarted would tell the fleet that a machine which is working fine has lost
        // the run — and `ps` would say so to the person who submitted it.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let elsewhere = NodeId::from_bytes([9; 32]);

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let epoch = run.assign(elsewhere, now(), LEASE_TTL).expect("assign");
        run.started(elsewhere, epoch, now()).expect("start");
        store.save_run(&run).expect("save");

        assert_eq!(sup.recover().expect("recover"), 0);
        assert_eq!(
            store
                .load_run(run.id)
                .expect("load")
                .expect("present")
                .state
                .name(),
            "running"
        );
    }

    #[tokio::test]
    async fn a_checkpointed_run_stays_pending_across_a_restart() {
        // `offload checkpoint` releases a run to the pool. Recovery used to fail every
        // non-terminal run indiscriminately, which would have destroyed exactly the runs
        // checkpointing exists to preserve — the ones with work waiting to be picked up.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let mut run = store.load_run(id).expect("load").expect("present");
        run.state = RunState::Pending {
            since: now(),
            let_go_by: None,
        };
        store.save_run(&run).expect("save");

        assert_eq!(
            supervisor_with(store.clone()).recover().expect("recover"),
            0,
            "a pending run has no holder to have lost"
        );
        assert_eq!(
            store
                .load_run(id)
                .expect("load")
                .expect("present")
                .state
                .name(),
            "pending",
            "and is still waiting for somebody to take it"
        );
    }

    #[tokio::test]
    async fn an_interrupted_run_with_a_checkpoint_says_it_can_be_resumed() {
        // The reason `ps` carries a failure reason at all: "failed" and "failed but every
        // turn is still on disk" call for completely different next actions.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let mut run = store.load_run(id).expect("load").expect("present");
        run.checkpoint = Some(checkpoint_at(7));
        store.save_run(&run).expect("save");

        supervisor_with(store.clone()).recover().expect("recover");

        let reason = failure_reason(&store.load_run(id).expect("load").expect("present"))
            .expect("a failed run has a reason");
        assert!(reason.contains("turn 7"), "{reason}");
        assert!(reason.contains("offload resume"), "{reason}");
    }

    #[tokio::test]
    async fn a_newer_event_withdraws_an_occurrence_this_node_meant_to_resume() {
        // ADR-0027. A rule fires one occurrence at a time (ADR-0020 §3) and auto-resume knows
        // nothing about rules, so the previous occurrence was picked back up beside its
        // successor — three more agent launches on an event the firing had superseded.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        run.checkpoint = Some(checkpoint_at(3));
        run.abandon("the agent reported failure", now())
            .expect("fail");
        store.save_run(&run).expect("save");

        // Nobody was watching, which is true of every occurrence by construction — so the run is
        // one tick away from a second agent. Asserted through the decision itself, because the
        // point of the withdrawal is that this is the *only* thing standing in its way.
        sup.note_failure_seen_as(id, Some(offload_core::Attendance::Unattended));
        let policy = offload_core::RecoveryPolicy::default();
        assert!(
            matches!(
                offload_core::decide_recovery(
                    &run,
                    now() + policy.backoff_per_resume + offload_core::Millis(1),
                    &policy,
                    offload_core::Circumstances {
                        attendance: Some(offload_core::Attendance::Unattended),
                        standing: offload_core::Standing::Ours,
                        departing: false,
                        revoked: false,
                        hosting: offload_core::Hosting::Allowed,
                        alone: false,
                        turns: 0,
                        resumes: 0,
                    },
                ),
                Some(offload_core::Recovery::Resume { .. })
            ),
            "without the withdrawal this run resumes"
        );

        assert!(
            sup.stop_recovering(id),
            "the watch was armed and is now gone"
        );
        assert!(
            !sup.stop_recovering(id),
            "and saying so twice withdraws nothing — the caller logs on the true answer only"
        );
        assert!(
            sup.recover_failed_runs(
                Capacity::runs(4),
                &policy,
                &|_| offload_core::Hosting::Allowed,
                false,
            )
            .await
            .resumed
            .is_empty(),
            "so the tick has nothing to pick up"
        );
    }

    #[tokio::test]
    async fn a_tasks_leg_is_stamped_so_its_log_can_be_found() {
        // **`server::log_source` asks `RunProgress::by` which machine serves a run's log**, on
        // the sound grounds that the leg which wrote the numbers wrote the log. A task writes no
        // numbers — no turns, no bill, no worktree summary (ADR-0019 §2) — so nothing stamped the
        // leg and the answer fell through to the run's *arbiter*, which for a task submitted here
        // and placed elsewhere is this node. `offload logs` then printed nothing at all: exit 0,
        // no output, the same silence session sixty-two removed for an agent run.
        //
        // Measured on two daemons before the fix — the task's whole output on the machine that
        // ran it, nothing on the machine it was submitted from, and `progress_by` NULL in both
        // rows.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_nominating_a_task(store.clone());
        let id = failed_task(&sup, &store);
        assert_eq!(
            sup.progress_leg(id),
            None,
            "the precondition: a task that has not run stamps nothing"
        );

        sup.restart_task(id, Capacity::runs(4))
            .await
            .expect("restart");
        // The stamp is written where the log starts, so it is there before the program exits —
        // which is the point: a task killed mid-output is still findable.
        for _ in 0..100 {
            if sup.progress_leg(id).is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            sup.progress_leg(id),
            Some(sup.node_id),
            "the leg that wrote the log has to say so"
        );
    }

    /// A supervisor whose owner has nominated one program, so a task can actually be spawned.
    ///
    /// `Config::default()` nominates nothing, which is the honest default and is why every task
    /// test before this one stopped at the record.
    fn supervisor_nominating_a_task(store: Store) -> Supervisor {
        let cfg = Config {
            state_dir: std::env::temp_dir().join(format!("offload-task-{}", std::process::id())),
            tasks: vec![crate::config::TaskConfig {
                id: "hello".into(),
                service: "webhook".into(),
                command: "/bin/echo".into(),
                args: vec!["nominated".into()],
                env: Vec::new(),
                description: String::new(),
            }],
            ..Config::default()
        };
        Supervisor::new(Arc::new(cfg), NodeId::from_bytes([1; 32]), store).with_private_ledger()
    }

    /// A task run that failed **here**, with the recovery watch armed the way the daemon arms
    /// it: through `Supervisor::fail`, so the attendance observation exists and the tick will
    /// look at it. A row written straight to the store is failed and invisible to the pass.
    fn failed_task(sup: &Supervisor, store: &Store) -> RunId {
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            RunSpec {
                work: Work::Task(offload_core::TaskWork {
                    service: offload_core::Service::Webhook,
                    args: Vec::new(),
                }),
                restartability: Restartability::Idempotent,
                ..spec(&RepoSource::parse("/tmp/whatever"))
            },
            sup.node_id,
            now(),
        );
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(id, epoch).expect("registered");
        sup.fail(id, "exit status 2".to_string());
        sup.release(id);
        id
    }

    #[tokio::test]
    async fn a_failed_task_is_restarted_rather_than_resumed() {
        // **`decide_recovery` says `Resume` for a failed task and means "run it again".** The
        // mechanism behind that answer was `resume`, which refuses a task for want of a
        // conversation — so the tick asked every thirty seconds, was refused instantly, counted
        // the refusal as a spent retry, and the program never ran a second time. Two mechanisms
        // for one decision, one of them missing (ADR-0058). Walked on two daemons before this
        // test existed; the walk is what found it, and this is what keeps the pair together.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_nominating_a_task(store.clone());
        let id = failed_task(&sup, &store);

        // The door a person types at still refuses, and that is a decision rather than a gap:
        // resuming is continuing a conversation, and what somebody wants for a failed task is
        // `offload run --task` (see `Supervisor::resume`'s own note).
        let refused = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("a task has no conversation to continue");
        assert!(refused.to_string().contains("no checkpoint"), "{refused}");

        // The tick's door starts the program again, at a fresh epoch.
        sup.restart_task(id, Capacity::runs(4))
            .await
            .expect("a failed task restarts from its spec");
        let after = sup.run(id).expect("row");
        assert!(
            !after.state.is_terminal(),
            "it should be held again, not left failed: {:?}",
            after.state
        );
        assert!(after.epoch > Epoch(1), "a restart is a fresh claim");

        // **And the tick reaches that door rather than the other one**, which is where the
        // defect actually lived: `decide_recovery` answers `Resume` for both tiers, and the arm
        // that acts on it had one mechanism. With no backoff to wait out, one pass is enough.
        let policy = offload_core::RecoveryPolicy {
            backoff_per_resume: Millis(0),
            min_backoff: Millis(0),
            ..offload_core::RecoveryPolicy::default()
        };
        let second = failed_task(&sup, &store);
        let picked = sup
            .recover_failed_runs(
                Capacity::runs(4),
                &policy,
                &|_| offload_core::Hosting::Allowed,
                false,
            )
            .await
            .resumed;
        assert!(
            picked.iter().any(|run| run.id == second),
            "the tick picked the task back up: {:?}",
            picked.iter().map(|r| r.id.short()).collect::<Vec<_>>()
        );
        assert!(
            !sup.run(second).expect("row").state.is_terminal(),
            "and it is running again rather than still failed"
        );

        // And it is the *task* door: an agent run reaching it is a bug elsewhere, answered with
        // the sentence `resume`'s tail gives a task, rather than with a program that does not
        // exist.
        let (agent, mut run) = dormant_run(&sup, &store);
        run.abandon("something broke", now()).expect("abandon");
        store.save_run(&run).expect("save");
        let wrong_tier = sup
            .restart_task(agent, Capacity::runs(4))
            .await
            .expect_err("this door is for the cheap tier");
        assert!(
            matches!(wrong_tier, SubmitError::UnsupportedWork { kind } if kind == "agent"),
            "{wrong_tier}"
        );
    }

    #[tokio::test]
    async fn resume_refuses_what_it_cannot_continue_and_says_which() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        // No checkpoint: there is no conversation to continue.
        let (id, mut run) = dormant_run(&sup, &store);
        run.abandon("something broke", now()).expect("abandon");
        store.save_run(&run).expect("save");

        let err = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("nothing to resume from");
        assert!(err.to_string().contains("no checkpoint"), "{err}");

        // Completed on purpose: reopening it would restart finished work.
        run.state = RunState::Completed { at: now() };
        run.checkpoint = Some(checkpoint_at(3));
        store.save_run(&run).expect("save");

        let err = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("already finished");
        assert!(matches!(err, SubmitError::Refused { .. }), "{err}");
        assert!(err.to_string().contains("new run"), "{err}");
    }

    /// The listing's advice and the door's answer are one predicate, so they cannot disagree.
    ///
    /// `recover` writes `resumable from turn N with 'offload resume <id>'` onto an interrupted
    /// run at startup and stores it. That is right about the *run* — its conversation and its
    /// uncommitted edits are there — and it has to keep saying so, because a permanent note
    /// about why a run stopped is the wrong home for a fact that changes by the hour. What was
    /// missing is the other half: whether *this machine* would take the command. A drained node,
    /// a revoked one, and one whose owner said `accept = "never"` each refuse it (ADR-0047), and
    /// `offload ps` went on printing the invitation.
    ///
    /// The fix pairs two facts at the reader, and what this pins is that the run-shaped half is
    /// the door's own predicate rather than a second list of states. `RunSummary::resumable`
    /// exists for the pairing and is asserted here against `refuse_unresumable` for every state
    /// a run can be in: exactly the states the door admits, and no others. A row that says
    /// `resumable` where the command would answer *it finished — start a new run instead* is a
    /// footnote about nothing, and one that stays silent where the command would have run is the
    /// hole this closes.
    #[tokio::test]
    async fn a_listing_calls_a_run_resumable_exactly_where_the_door_would_take_it() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        // Fresh rather than `dormant_run`, which hands back a run already running: this needs
        // the whole life of one, starting at the state a submission creates.
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );

        // Every state a run can be found in, reached the way the machine really reaches it
        // rather than by assigning the variant: a state nothing can produce is not one the
        // listing has to be right about, and a hand-built one can be a shape that never occurs.
        // Paired with the answer written out here rather than computed, which is the whole
        // value of the test: `refuse_unresumable` *calls* `resumable_state`, so comparing the
        // two would compare a function with itself and pass on any answer at all. What is
        // pinned is which states those two agree on.
        let mut states = vec![(run.state.clone(), true)];
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        states.push((run.state.clone(), false));
        run.started(sup.node_id, epoch, now()).expect("start");
        states.push((run.state.clone(), false));
        for reach in [
            // Somebody is holding all three: mid-turn on the way to a capture, out of contact
            // but reclaimable, or stopped on purpose.
            Run::request_checkpoint as fn(&mut Run, Millis) -> Result<(), _>,
            Run::orphan,
            Run::cancel,
        ] {
            let mut branch = run.clone();
            reach(&mut branch, now()).expect("a state this run can reach");
            states.push((branch.state.clone(), false));
        }
        // The two that resume: an interrupted run, and one a person parked. `Completed` is the
        // near miss — it has a checkpoint and a conversation, and reopening it would restart
        // work somebody's agent finished.
        let mut broken = run.clone();
        broken.abandon("something broke", now()).expect("abandon");
        states.push((broken.state.clone(), true));
        let mut finished = run.clone();
        finished
            .complete(sup.node_id, epoch, now())
            .expect("complete");
        states.push((finished.state.clone(), false));

        // Pinned, so the sweep cannot silently shrink to the states that already agree, and so
        // that a state added to `RunState` later fails here rather than defaulting to silence.
        assert_eq!(states.len(), 8, "every state a run can be found in");

        for (state, resumable) in states {
            let mut candidate = run.clone();
            candidate.state = state.clone();
            candidate.checkpoint = Some(checkpoint_at(3));
            assert_eq!(
                resumable_state(&candidate),
                resumable,
                "the listing is wrong about a run in `{}`",
                state.name()
            );
            // And the door, which reaches the same answer through the function
            // `Supervisor::resume` actually gates on. Only its state clause — everything after
            // it is about this run's checkpoint and this machine's room, neither of which a
            // listing claims to know. Structurally one predicate today; this is what fails if
            // somebody splits them again.
            assert_eq!(
                refuse_unresumable(&candidate, id, "resumed").is_ok(),
                resumable,
                "the door is wrong about a run in `{}`",
                state.name()
            );
        }
    }

    #[tokio::test]
    async fn a_failed_task_is_never_one_a_person_is_told_to_resume() {
        // The two predicates part company on exactly one thing, and the states are not it.
        // `resumable_state` is the door's, shared with `restart_task` because what a run has to
        // *be* for either door to reopen it is a property of the run. `resumable_by_hand` is the
        // reports', and a task is restarted from its spec and never resumed (ADR-0058).
        //
        // Measured on two daemons before this existed: a failed task on a node without
        // `host-runs` got the whole resume pairing from both reports — `offload ps`'s footnote
        // (*"a run above says it is resumable, which is true of the run"*) and `offload
        // explain`'s *"resume refused here right now: this node has not been granted
        // host-runs"*. Both named a fleet grant as the fix. On the node that **had** the grant,
        // `offload resume` on the same run answered *"it has no checkpoint — there is no
        // conversation to continue"*: a node-level, temporary-sounding refusal standing in front
        // of a run-level, permanent one.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = failed_task(&sup, &store);
        let task = store.load_run(id).expect("load").expect("the failed task");

        assert!(
            resumable_state(&task),
            "the states are shared on purpose: this is the door `restart_task` admits it by"
        );
        assert!(
            !resumable_by_hand(&task),
            "…and `offload resume` is not the command for it on any node"
        );

        // The control, in the same state, one tier over — otherwise this passes on a predicate
        // that answers `false` to everything.
        let mut agent = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let epoch = agent.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        agent.started(sup.node_id, epoch, now()).expect("start");
        agent.abandon("something broke", now()).expect("abandon");
        assert!(
            resumable_by_hand(&agent),
            "an interrupted agent run is exactly what the invitation is for"
        );
    }

    #[tokio::test]
    async fn a_finished_run_is_reported_as_finished_not_as_busy() {
        // Found live: the run's own state is the honest answer, and the in-memory handle
        // is only a hint. Reading the hint first told the user to "cancel it first" about
        // a run that had completed twenty minutes earlier.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        let epoch = run.epoch;
        run.checkpoint = Some(checkpoint_at(1));
        run.complete(sup.node_id, epoch, now()).expect("complete");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(id, epoch).expect("registered");

        let err = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("finished");
        assert!(err.to_string().contains("finished"), "{err}");
        assert!(!err.to_string().contains("still running"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_resumes_at_once_start_one_agent() {
        // `Supervisor::resume` has two callers with different intentions — a person typing
        // `offload resume`, and the recovery tick picking an unattended failure back up — and
        // nothing schedules them apart. Before this, it loaded the run, spent two full
        // `list_runs` scans on the capacity check, then assigned and saved: a second caller
        // loading inside that window assigns from the same copy, computes the *same* epoch, and
        // launches a second agent into the same worktree. Both legs then hold the run at the
        // epoch they agree on, so every later fence waves both through — which is the double
        // execution this design exists to avoid.
        //
        // **This test is a tripwire, not the proof, and it did not trip.** Two hundred aligned
        // attempts on a barrier, before the fix, never lost the update: the loser's `load_run`
        // landed after the winner's `save_run` every time and it was refused with "it is
        // assigned, so somebody is already holding it". That ordering is a photo finish this
        // machine happens to win, not a guarantee — which is why the fix is the store's own
        // `update_run` (read, decide and write under one lock) rather than a timing argument.
        // What is asserted here is the invariant either way: one starts, one is refused, and the
        // refusal says which.
        for _ in 0..200 {
            let store = Store::open_memory().expect("store");
            let sup = supervisor_with(store.clone());
            let (id, mut run) = dormant_run(&sup, &store);
            run.checkpoint = Some(checkpoint_here(&store, 2));
            run.abandon("daemon restarted", now()).expect("abandon");
            store.save_run(&run).expect("save");

            let gate = std::sync::Arc::new(tokio::sync::Barrier::new(2));
            let (a, b) = (sup.clone(), sup.clone());
            let (ga, gb) = (gate.clone(), gate.clone());
            let (ra, rb) = tokio::join!(
                tokio::spawn(async move {
                    ga.wait().await;
                    a.resume(id, None, Capacity::runs(4)).await
                }),
                tokio::spawn(async move {
                    gb.wait().await;
                    b.resume(id, None, Capacity::runs(4)).await
                }),
            );
            let (ra, rb) = (ra.expect("join"), rb.expect("join"));
            assert_eq!(
                ra.is_ok() as u32 + rb.is_ok() as u32,
                1,
                "exactly one resume may start an agent: a={ra:?} b={rb:?}"
            );
            // And the loser is told something true. It lost to a run that is now *held*, which
            // is the one refusal here that is about somebody else rather than about the run.
            let refusal = ra
                .err()
                .or(rb.err())
                .expect("one of them was refused")
                .to_string();
            assert!(
                refusal.contains("already holding it"),
                "the loser reads the winner's state: {refusal}"
            );
        }
    }

    /// A fleet that really does deliver the blob it is asked for.
    ///
    /// The counterpart to `GatedFetch`: what a `resume` on a node the checkpoint was never
    /// replicated to is supposed to meet, and the thing there was no code path to reach.
    #[derive(Debug)]
    struct Supplies {
        store: Store,
        bytes: Vec<u8>,
        fetched: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Peers for Supplies {
        async fn replicate(&self, _run: RunId, _blobs: Vec<offload_core::BlobHash>) -> Replicated {
            Replicated::NoPeer
        }

        async fn fetch(&self, blob: offload_core::BlobHash) -> Result<(), String> {
            self.fetched
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let stored = self
                .store
                .put_blob(&self.bytes)
                .map_err(|e| e.to_string())?;
            if stored == blob {
                Ok(())
            } else {
                Err("this peer has a different blob".into())
            }
        }
    }

    #[tokio::test]
    async fn a_resume_fetches_the_checkpoint_the_node_does_not_have() {
        // **The hole the whole of `Restorable` exists to close.** A run migrates to a node the
        // checkpoint was never replicated to — the case every walk so far avoided by having the
        // blobs there already — and the operator types `offload resume`. `start_run` fetched;
        // `resume`, which is both that command and the recovery tick, built the same
        // `Start::Resume` and went straight to `launch`. Measured on two daemons: the resume
        // failed instantly with `state store: blob 345e28fe… is not stored here`, with **no
        // blob-fetch line in the log at all**, because nothing had asked for one.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);

        // The transcript exists somewhere in the fleet; it does not exist here. That is the
        // whole of a migration to a node that was never a replica.
        let bytes = b"the conversation so far, on somebody else's disk".to_vec();
        let elsewhere = Store::open_memory().expect("peer store");
        let hash = elsewhere.put_blob(&bytes).expect("blob");
        assert!(!store.has_blob(hash), "the staging: it is not here");

        run.checkpoint = Some(Checkpoint {
            transcript: hash,
            ..checkpoint_at(4)
        });
        run.abandon("its node went away", now()).expect("abandon");
        store.save_run(&run).expect("save");

        // First with nobody to ask, which is the refusal — and it has to be a refusal at the
        // door rather than an `Ok` whose agent dies three awaits later inside the restore.
        let err = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("nothing to fetch from");
        let said = err.to_string();
        assert!(
            said.contains("not on this node") && said.contains("no fleet to ask"),
            "the sentence has to name the cause, not the store: {said}"
        );
        assert!(
            !said.contains("is not stored here"),
            "and it must not be a `StoreError` leaking out of a restore: {said}"
        );

        // Nothing was spent finding that out: the refusal is ahead of the claim, so the run is
        // exactly as it was and the next attempt is a first attempt.
        let after = store.load_run(id).expect("load").expect("present");
        assert_eq!(after.state.name(), "failed", "not reopened into `assigned`");
        assert_eq!(after.epoch, run.epoch, "and no epoch was burned");
        assert!(
            !lock(&sup.live).contains_key(&id),
            "and nothing is registered for an agent that was never launched"
        );

        // Now with a peer that has it. The fetch is the line that never appeared.
        let fetched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        sup.peers_via(Arc::new(Supplies {
            store: store.clone(),
            bytes,
            fetched: fetched.clone(),
        }));

        sup.resume(id, None, Capacity::runs(4))
            .await
            .expect("resumed, having fetched what it lacked");

        assert_eq!(
            fetched.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly the one blob it did not have"
        );
        assert!(store.has_blob(hash), "and it is here now");
    }

    #[tokio::test]
    async fn a_fetch_that_lies_about_succeeding_is_caught_before_the_agent_starts() {
        // The store is asked twice and the second answer decides. A `fetch` returning `Ok`
        // without leaving the bytes here is otherwise indistinguishable from one that worked,
        // and the place that would find out is `restore_workspace` — a worktree already half
        // built, and a `StoreError` for an operator to read.
        #[derive(Debug)]
        struct SaysYes;

        #[async_trait::async_trait]
        impl Peers for SaysYes {
            async fn replicate(
                &self,
                _run: RunId,
                _blobs: Vec<offload_core::BlobHash>,
            ) -> Replicated {
                Replicated::NoPeer
            }
            async fn fetch(&self, _blob: offload_core::BlobHash) -> Result<(), String> {
                Ok(())
            }
        }

        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        sup.peers_via(Arc::new(SaysYes));
        let (id, mut run) = dormant_run(&sup, &store);
        run.checkpoint = Some(checkpoint_at(4));
        run.abandon("its node went away", now()).expect("abandon");
        store.save_run(&run).expect("save");

        let err = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("the bytes are still not here");
        assert!(
            err.to_string().contains("still not here"),
            "and it says so rather than starting: {err}"
        );
        assert_eq!(
            store.load_run(id).expect("load").expect("present").epoch,
            run.epoch,
            "with nothing spent"
        );
    }

    #[tokio::test]
    async fn resuming_a_failed_run_reopens_it_at_a_fresh_epoch() {
        // The epoch has to move even on a single node: whatever was holding the run when
        // it failed must not be able to land side effects on it afterwards.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);

        let stale = run.epoch;
        run.checkpoint = Some(checkpoint_here(&store, 2));
        run.abandon("daemon restarted", now()).expect("abandon");
        store.save_run(&run).expect("save");

        sup.resume(id, None, Capacity::runs(4))
            .await
            .expect("resume");

        // Read the epoch, not the state: the run this spawns cannot start (the test's repo
        // path is not a repository) and will fail itself again shortly. The epoch is
        // monotonic regardless of how that race lands.
        assert!(
            store.load_run(id).expect("load").expect("present").epoch > stale,
            "reopening a run invalidates the epoch it failed under"
        );
    }

    #[tokio::test]
    async fn a_failed_run_picks_itself_up_only_if_nobody_was_watching() {
        // ADR-0013's third axis, both halves. The observation is taken when the run fails
        // rather than when the decision is made, and this test is the reason: the follower
        // below would be gone a moment later, because a failure ends the stream it was reading.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        // No backoff, so the decision is the only thing under test.
        let policy = offload_core::RecoveryPolicy {
            backoff_per_resume: Millis(0),
            min_backoff: Millis(0),
            ..offload_core::RecoveryPolicy::default()
        };

        let mut ids = Vec::new();
        for watched in [true, false] {
            let (id, mut run) = dormant_run(&sup, &store);
            run.checkpoint = Some(checkpoint_here(&store, 4));
            store.save_run(&run).expect("save");
            let _cancel = sup.register(id, run.epoch).expect("registered");
            let follower = watched.then(|| {
                sup.subscribe(id)
                    .expect("subscribe")
                    .1
                    .expect("a live run has a channel")
            });
            assert_eq!(sup.watchers(id), Some(u32::from(watched)));

            sup.fail(id, "agent reported failure".to_string());
            drop(follower); // exactly what a following client does when the stream ends
            ids.push((id, run.epoch));
        }

        let (watched, watched_epoch) = ids[0];
        let (alone, alone_epoch) = ids[1];
        sup.recover_failed_runs(
            Capacity::runs(4),
            &policy,
            &|_| offload_core::Hosting::Allowed,
            false,
        )
        .await;

        // Somebody was looking: it stays failed, and it is said once rather than every tick.
        let still = store.load_run(watched).expect("load").expect("present");
        assert!(
            still.state.is_failed(),
            "a watched failure is theirs to look at"
        );
        assert_eq!(still.epoch, watched_epoch, "and nothing touched it");
        // The decision is *written down* rather than forgotten, which is what "said once" needs
        // and what deleting the entry also destroyed: `offload explain` had nothing to print and
        // fell back to sampling the stream, which after a failure is always "nobody is watching".
        assert_eq!(
            lock(&sup.recovery).get(&watched).and_then(|r| r.decided),
            Some(offload_core::Escalation::SomebodyWasWatching),
        );
        assert_eq!(
            sup.recovery_state(watched).and_then(|s| s.observed),
            Some(offload_core::Attendance::Attended),
            "and so is the observation it was taken on, which cannot be re-taken"
        );

        // Said once: a second pass leaves it exactly as it is and does not re-decide.
        sup.recover_failed_runs(
            Capacity::runs(4),
            &policy,
            &|_| offload_core::Hosting::Allowed,
            false,
        )
        .await;
        let again = store.load_run(watched).expect("load").expect("present");
        assert_eq!(again.epoch, watched_epoch, "still nothing touched it");

        // …and a person taking it over clears both, which is where the fresh budget comes from.
        assert!(sup.stop_recovering(watched));
        assert!(sup.recovery_state(watched).is_none());

        // Nobody was: it picks itself back up. The epoch rather than the state, because the
        // run this spawns cannot start (the test's repo path is not a repository) and will fail
        // itself again shortly — the epoch is monotonic however that race lands.
        assert!(
            store.load_run(alone).expect("load").expect("present").epoch > alone_epoch,
            "an unattended failure resumes itself rather than waiting for a human"
        );
        assert_eq!(
            lock(&sup.recovery).get(&alone).map(|r| r.resumes),
            Some(1),
            "and the attempt is counted, so it cannot loop for ever"
        );
    }

    #[tokio::test]
    async fn a_run_that_moved_on_while_its_workspace_was_being_built_never_gets_an_agent() {
        // The failure this project says matters most, reached through the guard that exists to
        // prevent it. Everything before the spawn takes time somebody else can act in — a cold
        // clone runs for minutes, and restoring a workspace fetches blobs from peers, which is
        // exactly the slowness that gets a node concluded dead and its run reassigned. The epoch
        // check used to sit *after* `spawn`, and its failure was discarded: two agents, one repo,
        // both committing, and nothing in either log looking wrong.
        //
        // Arranged rather than raced: the run is registered at the epoch this leg believes in,
        // and then granted to somebody else, which is precisely the state a slow prepare returns
        // into.
        let dir = std::env::temp_dir().join(format!("offload-fence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        // If the fence lets anything through, this is what runs — and it would be recorded as a
        // started agent, which is the assertion below.
        config.agent.binary = std::path::PathBuf::from("/bin/true");
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        let cancel_rx = sup.register(id, ours).expect("registered");

        // While we were preparing: the arbiter gave it to somebody else at a higher epoch, and
        // this node learned that the ordinary way — a gossip merge into its own store.
        let elsewhere = NodeId::from_bytes([7; 32]);
        run.orphan(now()).expect("out of contact");
        let theirs = run.assign(elsewhere, now(), LEASE_TTL).expect("reassign");
        assert!(theirs > ours);
        store.save_run(&run).expect("save");

        let err = sup
            .drive(id, source, None, cancel_rx, Start::Fresh)
            .await
            .expect_err("a run this node no longer holds must not get an agent");
        assert!(
            err.to_string().contains("holder") || err.to_string().contains("epoch"),
            "the refusal has to say which fence stopped it: {err}"
        );

        // And the run is left exactly as its new holder has it. A leg that lost the run must not
        // write anything about it — that is the whole of what fencing buys.
        let after = store.load_run(id).expect("load").expect("present");
        assert_eq!(after.epoch, theirs);
        assert_eq!(after.holder(), Some(elsewhere));
        assert_eq!(
            after.state.name(),
            "assigned",
            "this node must not have moved it to running"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_run_granted_to_somebody_else_at_our_own_epoch_stops_the_agent_here() {
        // The fence at 3820 is the same guard on the way *in*; this is it on the way out, for
        // an agent that is already running. It used to test `live.epoch < run.epoch`, which is
        // the ordinary reassignment and leaves out the one case that matters most: two grants
        // at the *same* epoch, which is what two arbiters produce (they both bump `next()` from
        // one number) and what a round produced for every node in it until `hand_over` started
        // spending a token per attempt. Read as "not our business", it left two agents running
        // on one repository, both committing, for as long as the run lasted.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        let ours = run.epoch;
        let mut cancel_rx = sup.register(id, ours).expect("registered");
        // Twelve turns of work on this leg, which is the number somebody has been watching and
        // the number that is about to stop being the run's.
        sup.record_event(id, AgentEvent::TurnBoundary { turn: 12 });

        // The merge has settled the run against us: same epoch, somebody else's name on it.
        let mut theirs = run.clone();
        theirs.state = RunState::Running {
            lease: offload_core::Lease {
                node: NodeId::from_bytes([7; 32]),
                epoch: ours,
                expires_at: now() + LEASE_TTL,
            },
            started_at: now(),
        };
        assert_eq!(
            theirs.epoch, ours,
            "this is the equal-epoch case, not a reassignment"
        );
        sup.record_run(&theirs).expect("record");

        let stopped =
            tokio::time::timeout(std::time::Duration::from_secs(2), cancel_rx.recv()).await;
        assert!(
            matches!(stopped, Ok(Some(Halt::Superseded))),
            "the agent was left running while another node held the same run: {stopped:?}"
        );

        // And it is stopped as *superseded*, not as cancelled — the second half of the same
        // rule, and the one that was wrong. Losing a run and having it cancelled are different
        // facts, and `record_run` has already overwritten this row with the new holder's copy:
        // a `Cancelled` written here would be terminal, unfenced, and at *their* epoch, so it
        // would beat the live record on every node except the one actually running it. The
        // fleet would say cancelled while the agent worked on.
        assert_eq!(
            store.load_run(id).expect("load").expect("run").state.name(),
            "running",
            "this node closed a record that had just become somebody else's"
        );

        // And it is written down, on the one machine that can write it. The surviving leg never
        // hears that there was another one, so nothing else in the fleet knows this happened —
        // and the run's reported turn count has just dropped back to the survivor's, which is
        // correct and looks like a fault. This row is the answer to "why does `ps` say turn 1
        // when I watched it reach 12".
        let rows = store.audit(Some(id), 10).expect("audit");
        let superseded = rows
            .iter()
            .find(|e| e.kind.kind_name() == "superseded")
            .map(|e| e.kind.describe())
            .expect("losing a run is a thing this machine did");
        assert!(
            superseded.contains("turn 12") && superseded.contains(&NodeId::from_bytes([7; 32]).short()),
            "the row has to carry the number that was on screen and who has the run now: {superseded}"
        );
    }

    /// A leg that **finished** before it learned it had lost is written down too.
    ///
    /// Measured, session ninety: alpha frozen across its agent's last turn, bravo granted the run
    /// at epoch 2, alpha thawed, read the agent's result before the gossip, and completed the run
    /// at epoch 1. The merge put bravo's record over alpha's, and alpha's audit log said nothing —
    /// the `superseded` arm above needs a live agent, and this one had just exited. The control is
    /// a completed record whose leg ran *elsewhere*: overwriting that is ordinary gossip.
    #[test]
    fn a_leg_that_finished_and_then_lost_is_written_down() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let bravo = NodeId::from_bytes([7; 32]);
        let (id, mut ours) = dormant_run(&sup, &store);
        let held = ours.epoch;
        ours.state = RunState::Completed { at: now() };
        store.save_run(&ours).expect("save");
        let ran = |by| offload_core::RunProgress {
            by: Some(by),
            turns: 1,
            ..Default::default()
        };
        store.save_stats(id, &ran(sup.node_id)).expect("stats");

        let mut theirs = ours.clone();
        theirs.epoch = Epoch(held.0 + 1);
        theirs.state = RunState::Running {
            lease: offload_core::Lease {
                node: bravo,
                epoch: theirs.epoch,
                expires_at: now() + LEASE_TTL,
            },
            started_at: now(),
        };
        sup.record_run(&theirs).expect("record");
        let rows = store.audit(Some(id), 10).expect("audit");
        let line = rows
            .iter()
            .find(|e| e.kind.kind_name() == "superseded")
            .map(|e| e.kind.describe())
            .expect("a finished leg that lost is a thing this machine did");
        assert!(line.contains("after finishing"), "{line}");
        assert!(line.contains(&bravo.short()), "{line}");

        // The control: the same overwrite of a leg bravo ran is nobody's loss here.
        let (other, mut record) = dormant_run(&sup, &store);
        record.state = RunState::Completed { at: now() };
        store.save_run(&record).expect("save");
        store.save_stats(other, &ran(bravo)).expect("stats");
        let mut later = theirs.clone();
        later.id = other;
        sup.record_run(&later).expect("record");
        assert!(
            store.audit(Some(other), 10).expect("audit").is_empty(),
            "a leg another node ran is not this node's to have lost"
        );
    }

    #[tokio::test]
    async fn a_node_that_loses_a_run_stops_its_agent_without_closing_the_record() {
        // The half of the rule above that a registered-but-not-driven run cannot show. Stopping
        // an agent here and ending the run were one signal, so the *only* way to take an agent
        // down was the cancel channel and whatever came down it was recorded as `Cancelled`. A
        // node that had just lost a run therefore stopped its agent — right — and then wrote a
        // terminal state into the record `record_run` had a moment earlier overwritten with the
        // new holder's copy. Unfenced (an operator's cancel always wins), at *their* epoch, and
        // terminal, so it beat the live record everywhere `absorb` does not refuse a peer's word
        // about a run it holds. The fleet said cancelled; the agent worked on.
        let dir = std::env::temp_dir().join(format!("offload-superseded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        // An agent that starts, says nothing, and does not exit: what a real one looks like
        // between turns, and the only shape in which the pump can be interrupted at all.
        let binary = dir.join("quiet-agent");
        std::fs::write(&binary, "#!/bin/sh\nsleep 30\n").expect("write");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod");

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        config.agent.binary = binary;
        config.agent.cancel_grace_secs = 1;
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        let cancel_rx = sup.register(id, ours).expect("registered");

        let driving = {
            let sup = sup.clone();
            let source = source.clone();
            tokio::spawn(async move { sup.drive(id, source, None, cancel_rx, Start::Fresh).await })
        };

        // Wait for the agent to be up, which is where the interesting window is — and assert it
        // got there, because a test that raced past the spawn would pass without ever reaching
        // the code it is about.
        let mut up = false;
        for _ in 0..200 {
            if store
                .load_run(id)
                .expect("load")
                .is_some_and(|run| run.state.name() == "running")
            {
                up = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            up,
            "the agent never started, so nothing below is under test"
        );

        // The merge settles the run against us: same epoch, somebody else's name on it.
        let mut theirs = store.load_run(id).expect("load").expect("run");
        theirs.state = RunState::Running {
            lease: offload_core::Lease {
                node: NodeId::from_bytes([7; 32]),
                epoch: ours,
                expires_at: now() + LEASE_TTL,
            },
            started_at: now(),
        };
        sup.record_run(&theirs).expect("record");
        // What the fleet knows about the run's worktree once it is somebody else's — the new
        // holder's summary, arriving by gossip like any other number.
        sup.note_workspace(id, "2 modified (theirs)");

        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), driving).await;

        let after = store.load_run(id).expect("load").expect("run");
        assert_eq!(
            after.state.name(),
            "running",
            "this node closed a record that had just become somebody else's"
        );
        // And it said nothing about the run's worktree on the way out. These numbers gossip and
        // `accept_progress` breaks a tie on the author's clock, so the last write wins the fleet:
        // this leg publishing its own stale checkout would replace the new holder's, in the
        // column somebody reads to find out whether there is uncommitted work.
        assert_eq!(
            store.load_stats(id).expect("stats").workspace,
            "2 modified (theirs)",
            "a leg that lost the run described its own checkout as the run's"
        );
        assert_eq!(after.holder(), Some(NodeId::from_bytes([7; 32])));
        // And nothing terminal in the log either, which is the same mistake one layer down:
        // `LogEvent::is_terminal` hangs up every follower and ends the attendance that decides
        // whether a failure resumes itself.
        let logged = store
            .events_since::<LogEvent>(id, 0)
            .expect("events")
            .into_iter()
            .map(|(_, event)| event)
            .collect::<Vec<_>>();
        assert!(
            !logged.iter().any(offload_core::LogEvent::is_terminal),
            "a leg that lost its run told every follower the run was over: {logged:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_run_stops_at_the_turn_limit_it_was_given_and_does_not_go_back_in_the_pool() {
        // The field this test exists for was declared in phase 1, initialised to `None` in
        // eight fixtures, and read by nothing at all for seven phases — a control that reads as
        // being applied and is not, which is `WorkPolicy::allowed_agents` for the third time.
        //
        // Driven against a real pump rather than asserted on the arithmetic, because the
        // arithmetic was never the hard part: what had to be true is that the agent is *stopped*
        // at the boundary, that the run does not come back as `Pending` for the next node to
        // carry on with, and that the state it ends in is not one that reads as success.
        let dir = std::env::temp_dir().join(format!("offload-maxturns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        // An agent that would take six turns if nobody stopped it, and then keeps its process
        // alive. The `sleep` is the assertion's other half: if the limit did nothing, this run
        // would still be running when the timeout below expires, and the turn count would be
        // past the limit rather than on it.
        let binary = dir.join("eager-agent");
        // It writes its own transcript where the adapter will look for it — the config
        // directory is on its environment and the project slug is the cwd with the slashes
        // turned into dashes. Without that the capture at the limit has no conversation to
        // store and fails, which is a different behaviour from the one under test.
        let mut script = String::from(
            "#!/bin/sh\n slug=$(pwd | sed 's|/|-|g')\n mkdir -p \"$CLAUDE_CONFIG_DIR/projects/$slug\"\n printf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"content\":[{\"type\":\"text\",\"text\":\"t1\"}],\"usage\":{\"input_tokens\":5,\"output_tokens\":9}}}' > \"$CLAUDE_CONFIG_DIR/projects/$slug/s1.jsonl\"\n echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\", \"model\":\"fake\",\"tools\":[],\"cwd\":\".\"}'\n",
        );
        for turn in 1..=6 {
            script.push_str(&format!(
                " echo '{{\"type\":\"assistant\",\"message\":{{\"id\":\"m{turn}\",\"model\":\"fake\", \"content\":[{{\"type\":\"text\",\"text\":\"turn {turn}\"}}], \"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}}}'\n sleep 1\n"
            ));
        }
        script.push_str(" sleep 30\n");
        std::fs::write(&binary, script).expect("write");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod");

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        config.agent.binary = binary;
        config.agent.cancel_grace_secs = 1;
        // Automatic captures off, which is the setting that would leave a limited run with
        // nothing to show for itself if the limit's own capture were conditional on the cadence.
        config.checkpoint.every_turns = 0;
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut spec = spec(&source);
        spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(2);
        let mut run = Run::new(id, spec, sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        let cancel_rx = sup.register(id, ours).expect("registered");

        let driving = {
            let sup = sup.clone();
            let source = source.clone();
            tokio::spawn(async move { sup.drive(id, source, None, cancel_rx, Start::Fresh).await })
        };
        // Generous, and it has to be: the point is that the leg ends *by itself*. Nothing in
        // this test asks the agent to stop.
        let ended = tokio::time::timeout(std::time::Duration::from_secs(30), driving).await;
        assert!(ended.is_ok(), "the limit never stopped the run");

        let after = store.load_run(id).expect("load").expect("run");
        assert_eq!(
            store.load_stats(id).expect("stats").turns,
            2,
            "the run took turns it was not given"
        );

        // **Not `Pending`.** A checkpoint hands a run back to the pool, and a spent run handed
        // back is one the next bidder carries on past the limit — the cap applied on one machine
        // at a time and never to the run.
        assert_eq!(
            after.state.name(),
            "failed",
            "a run stopped by its own budget ended up {}",
            after.state.name()
        );
        // And not `completed`, which is the other half of the same sentence: `Completed` is
        // terminal *and* successful, so an abandoned run that reads as a success is what the
        // delivery plane would have told somebody about at breakfast.
        let RunState::Failed { ref reason, .. } = after.state else {
            panic!("expected a failure, got {:?}", after.state)
        };
        assert!(
            reason.contains("turn limit") && reason.contains('2'),
            "the reason has to name the limit, or `offload ps` cannot tell this from a crash: {reason:?}"
        );

        // The work survives. A run somebody capped is precisely the run they want to look at,
        // and this one was configured to take no automatic checkpoints at all.
        let cp = after.checkpoint.as_ref().expect("a capture at the limit");
        assert_eq!(cp.turns, 2, "and it is the capture taken at the limit");
        assert!(cp.session_id.is_some(), "with the conversation in it");

        // Which is exactly enough to resume from — and it is still refused, because the number
        // the operator gave is the number they gave. Without this the limit is a suggestion:
        // `offload resume` is offered by name in the sentence a failed run prints.
        let err = sup
            .resume(id, None, Capacity::runs(4))
            .await
            .expect_err("a spent run must not resume");
        let said = err.to_string();
        assert!(
            said.contains("turn limit"),
            "and it has to say why, rather than refusing for some other reason: {said}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_capture_at_the_turn_limit_waits_for_the_agent_to_write_the_turn() {
        // **The one capture in the system with no retry behind it.** Every other failed capture
        // is answered at the next turn boundary, which is what lets `checkpoint` refuse a
        // transcript holding no conversation and call the refusal harmless. The turn limit
        // captures at the boundary it *stops* on, so there is no next boundary and the refusal
        // is permanent: the run somebody capped — precisely the run they wanted to look at
        // (ADR-0039 §4) — ends with no checkpoint at all.
        //
        // Measured with a real agent before this guard existed: two of three `--max-turns 1`
        // runs ended with no `checkpoint` line in `offload explain` and a dash under SAFE in
        // `ps`, while `note_tokens` — which reads the same file later — filled the TOKENS column
        // in beside it. The same transcript, read twice, disagreeing about whether the
        // conversation was there.
        //
        // The agent below is that race made deterministic: the transcript holds no conversation
        // when the boundary is reached, and gains it a beat later. Without the wait this test
        // fails on the `expect` at the bottom, which is the whole point of it.
        let dir = std::env::temp_dir().join(format!("offload-limitflush-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        let binary = dir.join("flushing-agent");
        // Rows that are real and carry no conversation — the shape measured on claude 2.1.251,
        // where every transcript that had not caught up was the session preamble and nothing
        // else. The assistant row arrives from a background shell after the boundary has been
        // reached and the limit has fired.
        let script = concat!(
            "#!/bin/sh\n",
            " slug=$(pwd | sed 's|/|-|g')\n",
            " d=\"$CLAUDE_CONFIG_DIR/projects/$slug\"\n",
            " mkdir -p \"$d\"\n",
            " printf '%s\\n' '{\"type\":\"queue-operation\"}' > \"$d/s1.jsonl\"\n",
            " printf '%s\\n' '{\"type\":\"user\",\"message\":{\"role\":\"user\"}}' >> \"$d/s1.jsonl\"\n",
            " ( sleep 1; printf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"content\":[{\"type\":\"text\",\"text\":\"t1\"}],\"usage\":{\"input_tokens\":5,\"output_tokens\":9}}}' >> \"$d/s1.jsonl\" ) &\n",
            " echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\",\"model\":\"fake\",\"tools\":[],\"cwd\":\".\"}'\n",
            " echo '{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"fake\",\"content\":[{\"type\":\"text\",\"text\":\"turn 1\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}'\n",
            " sleep 30\n",
        );
        std::fs::write(&binary, script).expect("write");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod");

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        config.agent.binary = binary;
        config.agent.cancel_grace_secs = 1;
        // Off, so the only capture this run can possibly have is the limit's own.
        config.checkpoint.every_turns = 0;
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut spec = spec(&source);
        spec.agent_mut().expect("an agent run").max_turns = std::num::NonZeroU32::new(1);
        let mut run = Run::new(id, spec, sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        let cancel_rx = sup.register(id, ours).expect("registered");

        let driving = {
            let sup = sup.clone();
            let source = source.clone();
            tokio::spawn(async move { sup.drive(id, source, None, cancel_rx, Start::Fresh).await })
        };
        let ended = tokio::time::timeout(std::time::Duration::from_secs(30), driving).await;
        assert!(ended.is_ok(), "the limit never stopped the run");

        let after = store.load_run(id).expect("load").expect("run");
        assert_eq!(after.state.name(), "failed");

        // The assertion this test exists for. It is not about tokens or about the report — it is
        // that the conversation the operator capped is still reachable.
        let cp = after
            .checkpoint
            .as_ref()
            .expect("the capture at the limit waited for the agent to write the turn");
        assert_eq!(cp.turns, 1);
        assert!(cp.session_id.is_some(), "with the conversation in it");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_running_run_reports_what_its_worktree_holds_rather_than_preparing() {
        // `RunProgress::workspace` is documented as "the holder's summary of the run's worktree
        // — 2 modified, 1 new", and it was written exactly twice in a run's life: `preparing`
        // at launch, and the truth once the leg had already ended. So `offload ps` said
        // `preparing` for the whole of an overnight run — beside the column saying whether that
        // work is replicated, which is the pair somebody checks at 07:00 — and it *gossips*, so
        // every node in the fleet reported a worktree as half-built while an agent worked in it.
        let dir = std::env::temp_dir().join(format!("offload-wsum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        // An agent that writes a file, ends a turn (a text-only reply is a boundary), and then
        // stays up — which is what a real one looks like between turns, and the only shape in
        // which a *running* run can be inspected at all.
        let binary = dir.join("writing-agent");
        std::fs::write(
            &binary,
            "#!/bin/sh\n             echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\", \"model\":\"fake\",\"tools\":[],\"cwd\":\".\"}'\n echo hello > invented.rs\n echo '{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"fake\", \"content\":[{\"type\":\"text\",\"text\":\"done\"}], \"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}'\n sleep 30\n",
        )
        .expect("write");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod");

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        config.agent.binary = binary;
        config.agent.cancel_grace_secs = 1;
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        // What `start_run` writes on the way in, so the test begins in the state a real run
        // begins in. It is true while a cold clone runs for minutes, and was still there hours
        // later — this is the value under test, not scaffolding.
        sup.note_workspace(id, "preparing");
        let cancel_rx = sup.register(id, ours).expect("registered");

        let driving = {
            let sup = sup.clone();
            let source = source.clone();
            tokio::spawn(async move { sup.drive(id, source, None, cancel_rx, Start::Fresh).await })
        };

        // Wait for the turn to land — the run is still running afterwards, which is the state
        // under test. Deliberately *not* waiting for the leg to end: the summary was always
        // right by then, which is exactly why this went unnoticed.
        let mut summary = String::new();
        for _ in 0..200 {
            let stats = store.load_stats(id).expect("stats");
            if stats.turns >= 1 {
                summary = stats.workspace;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(
            store.load_run(id).expect("load").expect("run").state.name(),
            "running",
            "the point is what a run reports while it is still going"
        );
        assert_ne!(
            summary, "preparing",
            "a run a turn into its work said its worktree was still being built"
        );
        assert!(
            summary.contains("new"),
            "and it should name the file the agent invented: {summary:?}"
        );
        // And the numbers say which leg wrote them, which is what lets the fleet rank this
        // position against another leg's rather than merely find it larger. Asserted here, on a
        // real agent's real turn boundary, because the stamp comes from the live leg in memory
        // and a unit test can only check the arithmetic that reads it.
        let stamped = store.load_stats(id).expect("stats");
        assert_eq!(stamped.by, Some(sup.node_id));
        assert_eq!(stamped.epoch, ours, "and under the epoch it is running at");

        sup.halt_agent(id, Halt::Superseded).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), driving).await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_leg_that_was_told_it_lost_the_run_records_no_refused_write() {
        // Found by draining a real two-node fleet and reading `offload audit` on the machine
        // that left. Every graceful drain left a `refused: this node held epoch 1 and tried to
        // describe it` row on the departing node, one line under the `superseded` row that says
        // the same event correctly — because the departing leg is superseded a moment after
        // handing its run over, so `describes` is false by the time the leg ends.
        //
        // `Refused` rows are documented as "the rows worth having … every one of them is a
        // moment when two agents on one repository was prevented", which is exactly the property
        // that one-per-migration destroys: closing a laptop is the ordinary path this product is
        // named for, and it was reporting a fence. `Halt::Superseded` already says "nothing here
        // is written down"; this is the one place that did not honour it.
        let dir = std::env::temp_dir().join(format!("offload-drainaudit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        let binary = dir.join("quiet-turn-agent");
        std::fs::write(
            &binary,
            "#!/bin/sh\n echo '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\", \"model\":\"fake\",\"tools\":[],\"cwd\":\".\"}'\n echo '{\"type\":\"assistant\",\"message\":{\"id\":\"m1\",\"model\":\"fake\", \"content\":[{\"type\":\"text\",\"text\":\"done\"}], \"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}'\n sleep 30\n",
        )
        .expect("write");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod");

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        config.agent.binary = binary;
        config.agent.cancel_grace_secs = 1;
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        let cancel_rx = sup.register(id, ours).expect("registered");

        let driving = {
            let sup = sup.clone();
            let source = source.clone();
            tokio::spawn(async move { sup.drive(id, source, None, cancel_rx, Start::Fresh).await })
        };

        // Wait for the leg to be genuinely under way, so the describe at the end of it is a real
        // one rather than a leg that never started.
        for _ in 0..200 {
            if store.load_stats(id).expect("stats").turns >= 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // The fleet moved the run on: this is what a drain looks like from the departing node,
        // and what `record_run` acts on before asking the agent here to stop.
        let elsewhere = NodeId::from_bytes([9; 32]);
        let mut theirs = store.load_run(id).expect("load").expect("run");
        theirs.orphan(now()).expect("out of contact");
        let epoch = theirs
            .assign(elsewhere, now(), LEASE_TTL)
            .expect("reassign");
        theirs.started(elsewhere, epoch, now()).expect("start");
        store.save_run(&theirs).expect("save");

        sup.halt_agent(id, Halt::Superseded).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), driving).await;

        let entries = store.audit(Some(id), 20).expect("audit");
        assert!(
            !entries.iter().any(|e| matches!(
                e.kind,
                offload_core::AuditEvent::Refused {
                    attempt: offload_core::Attempt::Describe,
                    ..
                }
            )),
            "a graceful hand-over reported a fence: {entries:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_commitment_that_never_started_is_cancelled_rather_than_called_not_running() {
        // `offload cancel` used to answer from the process table, which cannot tell a run that
        // is somewhere else from a run that is *here* and has not begun. A node may accept work
        // it has no room for yet (ADR-0006), so this state is ordinary rather than exotic — and
        // the old answer, "run ab12… is not running", was a false statement about a run this
        // very node was holding and about to start.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        assert_eq!(run.state.name(), "assigned", "held here, no agent");
        store.save_run(&run).expect("save");

        let note = sup.cancel_run(id, "phone").await.expect("cancelled");
        assert!(note.contains("had not started"), "{note}");
        assert_eq!(
            store.load_run(id).expect("load").expect("run").state.name(),
            "cancelled"
        );
        // And the log says where the decision came from, which is the only place it is written
        // down — the same reason an answer carries the node it was typed at.
        let logged = store
            .events_since::<LogEvent>(id, 0)
            .expect("events")
            .into_iter()
            .map(|(_, event)| event.kind)
            .collect::<Vec<_>>();
        assert!(
            logged
                .iter()
                .any(|kind| matches!(kind, LogKind::Cancelled { by } if by == "phone")),
            "{logged:?}"
        );
    }

    #[tokio::test]
    async fn a_run_nobody_holds_is_cancelled_from_its_record() {
        // The third entrance to this command: a queued run (ADR-0014's `--queue`) has no agent
        // anywhere, so "is it running" is the wrong question — what is being stopped is a record
        // that would otherwise keep being offered.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let run = queued_run(&sup, &store, None);
        assert_eq!(run.state.name(), "pending");

        let note = sup.cancel_run(run.id, "here").await.expect("cancelled");
        assert!(note.contains("waiting to be placed"), "{note}");
        assert_eq!(
            store
                .load_run(run.id)
                .expect("load")
                .expect("run")
                .state
                .name(),
            "cancelled"
        );
    }

    #[tokio::test]
    async fn a_run_on_another_machine_is_refused_here_rather_than_written_terminal() {
        // The guard that makes forwarding safe to build on. `Run::cancel` is unfenced by design
        // — an operator action always wins — so a node that wrote it for somebody else's run
        // would produce a terminal state at the holder's own epoch, which beats a live record
        // everywhere `absorb` does not refuse it. The agent would keep working and the fleet
        // would say it had stopped: the original bug, with everybody convinced instead of one
        // operator.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let desktop = NodeId::from_bytes([9; 32]);
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let epoch = run.assign(desktop, now(), LEASE_TTL).expect("assign");
        run.started(desktop, epoch, now()).expect("start");
        store.save_run(&run).expect("save");

        let refused = sup
            .cancel_run(id, "here")
            .await
            .expect_err("not ours to close");
        assert!(refused.to_string().contains(&desktop.short()), "{refused}");
        assert_eq!(
            store.load_run(id).expect("load").expect("run").state.name(),
            "running",
            "a cancel typed on the wrong machine wrote a terminal state anyway"
        );
    }

    #[tokio::test]
    async fn a_cancel_that_lost_the_race_does_not_say_the_run_was_cancelled() {
        // The window somebody is most likely to be typing in: the agent reports its result, the
        // run goes `Completed`, and the event stream has not closed yet — so the next turn of the
        // pump's select loop, biased towards the cancel channel, picks up a cancel that arrived a
        // moment ago. `Run::cancel` refuses a terminal run, correctly, and the log line was
        // written anyway: `LogKind::Cancelled` is terminal to everything downstream, so a run
        // that had just succeeded had "cancelled" as the last word in its own log while its
        // record said `completed`.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        let epoch = run.epoch;
        run.complete(sup.node_id, epoch, now()).expect("complete");
        store.save_run(&run).expect("save");

        sup.finish_cancelled(id, "phone");

        assert_eq!(
            store.load_run(id).expect("load").expect("run").state.name(),
            "completed",
            "an unfenced cancel must still refuse a run that is over"
        );
        let logged = store
            .events_since::<LogEvent>(id, 0)
            .expect("events")
            .into_iter()
            .map(|(_, event)| event.kind)
            .collect::<Vec<_>>();
        assert!(
            !logged
                .iter()
                .any(|kind| matches!(kind, LogKind::Cancelled { .. })),
            "the run's own log says it was cancelled: {logged:?}"
        );
    }

    #[tokio::test]
    async fn a_run_that_has_already_finished_is_refused_as_a_race() {
        // Most of a second passes while a forwarded cancel is in flight, so "it finished first"
        // is an ordinary outcome rather than a fault — and it has to read as one.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        let epoch = run.epoch;
        run.complete(sup.node_id, epoch, now()).expect("complete");
        store.save_run(&run).expect("save");

        let refused = sup.cancel_run(id, "here").await.expect_err("already over");
        assert!(
            refused.to_string().contains("already completed"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn an_agent_that_died_after_the_run_moved_does_not_fail_it_for_the_new_holder() {
        // The third door into the same room. `fail_if_unfinished` exists because an agent that
        // dies without a result leaves a run `Running` for ever — real, and the fix for it asked
        // the wrong question: `still_ours` tested the run's *state* (`Running` or
        // `Checkpointing`) and not whose it is. A reassigned run is `Running` too — on somebody
        // else's machine — so a leg whose agent crashed at the moment it lost the run wrote
        // `Failed` over the new holder's record, unfenced, at the new holder's epoch.
        //
        // Reachable by ordinary luck: `record_run` asks the agent here to stop, and if the agent
        // dies of its own accord first the pump reports `Finished` rather than `Halted`.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        let ours = run.epoch;
        let _cancel_rx = sup.register(id, ours).expect("registered");

        // Reassigned, and learned the ordinary way: the record here is now theirs.
        let elsewhere = NodeId::from_bytes([7; 32]);
        let mut theirs = run.clone();
        theirs.orphan(now()).expect("out of contact");
        let epoch = theirs
            .assign(elsewhere, now(), LEASE_TTL)
            .expect("reassign");
        theirs.started(elsewhere, epoch, now()).expect("start");
        assert_eq!(theirs.state.name(), "running", "theirs, and running");
        store.save_run(&theirs).expect("save");

        sup.fail_if_unfinished(id);

        let after = store.load_run(id).expect("load").expect("present");
        assert_eq!(
            after.state.name(),
            "running",
            "a dead agent here failed a run that had moved to another machine"
        );
        assert_eq!(after.holder(), Some(elsewhere));
    }

    #[tokio::test]
    async fn a_refused_write_is_written_down_where_somebody_can_read_it() {
        // The audit log's whole reason: a fence firing is the most important event in this design
        // and it used to leave nothing behind but a `tracing` line on a machine nobody is logged
        // into. Every one of these rows is a moment when two agents on one repository was
        // *prevented* rather than merely unlikely — and the fenced-out leg is the only node that
        // knows, which is why the log is per-node and not gossiped.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        let _cancel_rx = sup.register(id, run.epoch).expect("registered");

        // Reassigned: the record here is now somebody else's.
        let elsewhere = NodeId::from_bytes([7; 32]);
        let mut theirs = run.clone();
        theirs.orphan(now()).expect("out of contact");
        let epoch = theirs
            .assign(elsewhere, now(), LEASE_TTL)
            .expect("reassign");
        theirs.started(elsewhere, epoch, now()).expect("start");
        store.save_run(&theirs).expect("save");

        sup.fail(id, "the workspace would not build".to_string());

        let entries = store.audit(Some(id), 10).expect("audit");
        assert!(
            entries.iter().any(|e| matches!(
                e.kind,
                offload_core::AuditEvent::Refused {
                    attempt: offload_core::Attempt::Fail,
                    ..
                }
            )),
            "the refusal left no trace: {entries:?}"
        );
        // And the row says what was attempted rather than just "fenced", which is the difference
        // between a log somebody can act on and one they have to guess at.
        let line = entries[0].kind.describe();
        assert!(line.contains("fail it"), "{line}");

        // The run's own state is untouched, which is the fix the row is evidence of.
        assert_eq!(
            store.load_run(id).expect("load").expect("run").state.name(),
            "running"
        );
    }

    #[tokio::test]
    async fn a_run_that_ended_here_may_still_be_described() {
        // The distinction `describes` exists for, and the false audit row that found it. A
        // finished run has **no holder** — the lease goes with the terminal transition — so a
        // holder check alone calls the machine that just ran it a stranger: no final worktree
        // summary (which is the "3 modified" somebody reads afterwards) and, worse, an audit row
        // saying a write was *refused*, which is a fence somebody would go looking for.
        //
        // The epoch is what tells "ended here" from "moved elsewhere", because the terminal
        // transitions are fenced: no other node could have written one under our number.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        let ours = run.epoch;
        let _cancel_rx = sup.register(id, ours).expect("registered");

        assert!(sup.describes(id), "running here");
        assert!(sup.holds(id), "and ours to conclude");

        run.complete(sup.node_id, ours, now()).expect("complete");
        store.save_run(&run).expect("save");
        assert_eq!(run.holder(), None, "a finished run names no node");

        assert!(
            sup.describes(id),
            "the machine that just ran it may still say what its worktree holds"
        );
        assert!(
            !sup.holds(id),
            "but there is nothing left to conclude, so it is not ours to fail"
        );

        // And a run that moved is neither, at the same epoch or a higher one.
        let elsewhere = NodeId::from_bytes([7; 32]);
        let mut theirs = run.clone();
        theirs.state = RunState::Running {
            lease: offload_core::Lease {
                node: elsewhere,
                epoch: ours,
                expires_at: now() + LEASE_TTL,
            },
            started_at: now(),
        };
        store.save_run(&theirs).expect("save");
        assert!(!sup.describes(id), "not ours to describe");
        assert!(!sup.holds(id), "and not ours to conclude");
    }

    #[tokio::test]
    async fn an_assignment_is_recorded_from_both_ends() {
        // "Why is my run *there*" has no durable answer today — `offload explain` re-asks the
        // fleet on purpose, because a bid describes one second. The grant does not: it is a
        // decision, with an epoch on it, and two grants at one epoch from one arbiter is the
        // failure the whole scheme exists to prevent. Both halves are recorded because silence
        // is a decline (ADR-0006): a node that took a grant whose answer was lost looks exactly
        // like a node that never got it, and the two rows are what tell them apart.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let elsewhere = NodeId::from_bytes([9; 32]);
        let epoch = run.assign(elsewhere, now(), LEASE_TTL).expect("assign");

        sup.note_granted(id, elsewhere, epoch);
        sup.note_accepted(id, epoch);

        let entries = store.audit(Some(id), 10).expect("audit");
        assert_eq!(entries.len(), 2, "{entries:?}");
        // Newest first, because the question is almost always about what just happened.
        assert!(matches!(
            entries[0].kind,
            offload_core::AuditEvent::Accepted { .. }
        ));
        assert!(matches!(
            entries[1].kind,
            offload_core::AuditEvent::Granted { to, .. } if to == elsewhere
        ));
        assert!(
            entries[1]
                .kind
                .describe()
                .contains(&format!("epoch {}", epoch.0)),
            "the epoch is the point: {}",
            entries[1].kind.describe()
        );

        // And it outlives the run, which is the reason `audit.run_id` carries no foreign key: the
        // rows worth reading are about runs this node has stopped holding.
        store.delete_run(id).expect("delete");
        assert_eq!(store.audit(Some(id), 10).expect("audit").len(), 2);
    }

    #[tokio::test]
    async fn a_leg_the_fence_refused_does_not_mark_somebody_else_s_run_failed() {
        // The test below drives `drive` directly and checks it never spawns an agent, which is
        // the guard working. This checks what the *caller* then does with the refusal, and that
        // is a different question: `launch` treats every error from `drive` as "this run failed"
        // and calls `fail`, which uses `abandon` — unfenced on purpose, because a run whose
        // workspace could not be built has no holder to fence against.
        //
        // For the one error that means *this node lost the run*, that is exactly wrong. `abandon`
        // writes `Failed` into the row as it stands, which by then is the new holder's record at
        // the new holder's epoch: terminal, so it beats the live record everywhere, and
        // `note_failure` then makes the run a candidate for auto-resume. Somebody else is running
        // it, the fleet says it failed, and recovery is entitled to start a second agent — the
        // double execution the fence exists to prevent, reached through the fence firing.
        let dir = std::env::temp_dir().join(format!("offload-lostfence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let source = git_repo(&dir.join("repo")).await;

        let store = Store::open_memory().expect("store");
        let mut config = Config {
            state_dir: dir.join("state"),
            ..Config::default()
        };
        config.agent.binary = std::path::PathBuf::from("/bin/true");
        let sup = Supervisor::new(Arc::new(config), NodeId::from_bytes([1; 32]), store.clone())
            .with_private_ledger();

        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(id, spec(&source), sup.node_id, now());
        let ours = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        store.save_run(&run).expect("save");
        let cancel_rx = sup.register(id, ours).expect("registered");

        // Reassigned while this leg was preparing, and learned the ordinary way.
        let elsewhere = NodeId::from_bytes([7; 32]);
        run.orphan(now()).expect("out of contact");
        let theirs = run.assign(elsewhere, now(), LEASE_TTL).expect("reassign");
        assert!(theirs > ours);
        store.save_run(&run).expect("save");

        sup.launch(id, source, None, cancel_rx, Start::Fresh);

        // The leg has to get far enough to be refused; polling rather than a fixed sleep, and
        // the assertion is what the record says once it has.
        for _ in 0..200 {
            if lock(&sup.live).get(&id).is_none_or(|l| l.cancel.is_none()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let after = store.load_run(id).expect("load").expect("present");
        assert_eq!(
            after.state.name(),
            "assigned",
            "a leg that lost the run wrote a terminal state over the holder's record"
        );
        assert_eq!(after.holder(), Some(elsewhere));
        assert_eq!(after.epoch, theirs);
        assert!(
            lock(&sup.recovery).get(&id).is_none(),
            "and it must not be queued for auto-resume, which would be the second agent"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_agent_that_dies_without_a_result_fails_the_run_rather_than_leaving_it_running() {
        // Found by pointing `agent.binary` at `/bin/false`. The event stream simply ended, so
        // no `Result` event ever moved the run out of `Running` — and its holder was alive and
        // renewing the lease, so nothing orphaned it either. `ps` said `running` for ever, and
        // recovery cannot recover what is never marked failed.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        assert_eq!(run.state.name(), "running");

        sup.fail_if_unfinished(id);
        let after = store.load_run(id).expect("load").expect("present");
        assert!(after.state.is_failed());
        assert!(
            failure_reason(&after).is_some_and(|r| r.contains("without reporting a result")),
            "the reason has to say what happened, not just that something did"
        );

        // And a run that said its piece is left exactly as it is: the stream ending *after* a
        // result is the normal case, and failing it there would overwrite the truth.
        let (done_id, mut done) = dormant_run(&sup, &store);
        done.complete(sup.node_id, done.epoch, now()).expect("done");
        store.save_run(&done).expect("save");
        sup.fail_if_unfinished(done_id);
        assert_eq!(
            store
                .load_run(done_id)
                .expect("load")
                .expect("present")
                .state
                .name(),
            "completed"
        );
    }

    #[tokio::test]
    async fn a_peer_asking_for_a_run_s_log_counts_as_somebody_watching_it() {
        // What makes attendance work for the runs this project exists to move. The person is at
        // their laptop and the agent is on the desktop, so the only thing the desktop can
        // observe is a peer that keeps asking for the log — and that is exactly what a follower
        // elsewhere *is* (ADR-0013). Without this, a run somebody was watching from another
        // machine would look unattended and resume itself behind them.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        let _cancel = sup.register(id, run.epoch).expect("registered");
        let laptop = NodeId::from_bytes([42; 32]);

        assert_eq!(sup.watchers(id), Some(0), "nobody yet");
        sup.note_remote_watcher(id, laptop);
        assert_eq!(sup.watchers(id), Some(1), "a peer is following it");

        // And the run it is watching is the one that counts: a follower on another run tells
        // this one nothing.
        let (other, other_run) = dormant_run(&sup, &store);
        let _other_cancel = sup.register(other, other_run.epoch).expect("registered");
        assert_eq!(sup.watchers(other), Some(0));

        // A peer that has gone away stops counting, without anything having to tell us: the
        // observation is "asked recently", because a poll is what a remote follower has instead
        // of a connection.
        lock(&sup.watchers).insert(
            (id, laptop),
            std::time::Instant::now() - (WATCHER_GRACE + std::time::Duration::from_secs(1)),
        );
        assert_eq!(sup.watchers(id), Some(0));
    }

    #[tokio::test]
    async fn a_log_page_says_whether_there_is_more_to_come() {
        // The distinction a follower cannot survive without: an empty page means *nothing new
        // yet*, and a run between turns is quiet for minutes. Only the end of the log ends the
        // following.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        let _cancel = sup.register(id, run.epoch).expect("registered");

        sup.append(id, LogKind::Text { text: "hi".into() });
        sup.append(id, LogKind::TurnBoundary { turn: 1 });

        let (page, done) = sup.events_after(id, 0, 128).expect("read");
        assert_eq!(page.len(), 2);
        assert!(!done, "the agent is still here, so there may be more");

        // From where the last page ended: nothing new, and still not over.
        let (tail, done) = sup.events_after(id, page[1].seq, 128).expect("read");
        assert!(tail.is_empty());
        assert!(!done);

        // The end of the run is the end of the log, and it says so on the event rather than
        // waiting for the record to catch up.
        sup.append(
            id,
            LogKind::Finished {
                success: true,
                turns: 1,
                denials: 0,
                cost_micro_usd: 1,
                work: offload_core::WorkKind::Agent,
                result: None,
            },
        );
        let (last, done) = sup.events_after(id, page[1].seq, 128).expect("read");
        assert_eq!(last.len(), 1);
        assert!(done);

        // A page that filled up is "more right now", which is not the same answer.
        let (first, done) = sup.events_after(id, 0, 1).expect("read");
        assert_eq!(first.len(), 1);
        assert!(!done, "a full page never ends a follow");
    }

    #[tokio::test]
    async fn a_run_interrupted_by_a_restart_waits_for_a_person() {
        // The tempting shortcut is that nobody can be watching after a restart — every client
        // was disconnected by it — so every interrupted run should resume itself. A daemon in a
        // crash loop would then restart every run on every start, with no memory of having done
        // it, and the bill arrives before anybody notices. So a restart *cannot tell*, and the
        // reason is said rather than the run silently sitting there.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        run.checkpoint = Some(checkpoint_at(6));
        store.save_run(&run).expect("save");

        let restarted = supervisor_with(store.clone());
        assert_eq!(restarted.recover().expect("recover"), 1);

        let epoch = store.load_run(id).expect("load").expect("present").epoch;
        restarted
            .recover_failed_runs(
                Capacity::runs(4),
                &offload_core::RecoveryPolicy::default(),
                &|_| offload_core::Hosting::Allowed,
                false,
            )
            .await;

        let after = store.load_run(id).expect("load").expect("present");
        assert!(after.state.is_failed(), "left for `offload resume`");
        assert_eq!(after.epoch, epoch, "and nothing acted on it");
    }

    #[tokio::test]
    async fn a_draining_node_hands_its_failed_runs_back_instead_of_stranding_them() {
        // ADR-0043, and the case ADR-0042 named and left open. ADR-0034 stopped a drained node
        // spawning agents for its own failed runs, which was right and left the run stopped in
        // front of a fleet that could continue it: `supervise` reads failed as terminal, nobody
        // bids on it, and the node that could say otherwise had said `NodeIsDeparting` and moved
        // on. The fix is a transition, not a start — the node this run leaves is still the node
        // that starts nothing.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        // No backoff: on the way out there is nothing to wait for, and this test is about the
        // decision rather than the clock.
        let policy = offload_core::RecoveryPolicy {
            backoff_per_resume: Millis(0),
            min_backoff: Millis(0),
            ..offload_core::RecoveryPolicy::default()
        };

        // A replica somewhere else, because that is what makes the offer honest: the fleet has
        // to be able to fetch the conversation it is being handed.
        let mut replicated = checkpoint_at(4);
        replicated
            .replicas
            .insert(offload_core::NodeId::from_bytes([9; 32]));

        let (id, mut run) = dormant_run(&sup, &store);
        run.checkpoint = Some(replicated.clone());
        store.save_run(&run).expect("save");
        let _cancel = sup.register(id, run.epoch).expect("registered");
        let epoch = run.epoch;

        // The one whose only copy is here. Same run in every other respect.
        let (alone, mut solo) = dormant_run(&sup, &store);
        solo.checkpoint = Some(checkpoint_at(4));
        store.save_run(&solo).expect("save");
        let _solo_cancel = sup.register(alone, solo.epoch).expect("registered");
        let solo_epoch = solo.epoch;

        // Nobody watching either of them — a failure ends the stream, which is why attendance is
        // observed here rather than sampled at the decision.
        sup.fail(id, "agent reported failure".to_string());
        sup.fail(alone, "agent reported failure".to_string());
        sup.stop_accepting();

        sup.recover_failed_runs(
            Capacity::runs(4),
            &policy,
            &|_| offload_core::Hosting::Allowed,
            false,
        )
        .await;

        let handed = store.load_run(id).expect("load").expect("present");
        assert_eq!(
            handed.let_go_by(),
            Some(sup.node_id),
            "the fleet is told who dropped it, which is what makes `supervise` offer it"
        );
        assert!(handed.epoch > epoch, "the leg that failed cannot act on it");
        assert_eq!(
            handed.checkpoint.as_ref().map(|c| c.turns),
            Some(4),
            "and the conversation goes with it"
        );
        assert!(
            lock(&sup.recovery).get(&id).is_none(),
            "this node's memory of retrying it here is over, because here is over"
        );

        // And the honest half: no copy anywhere else, so it stays `Failed` with the reason a
        // person can act on rather than being offered to a fleet that could not start it.
        let kept = store.load_run(alone).expect("load").expect("present");
        assert!(kept.state.is_failed());
        assert_eq!(kept.epoch, solo_epoch, "nothing acted on it");
        assert_eq!(
            lock(&sup.recovery).get(&alone).and_then(|r| r.decided),
            Some(offload_core::Escalation::NoCopyElsewhere),
        );

        // The property ADR-0034 bought and this must not have spent: whatever else happened, a
        // draining node started no agent. Both runs left `live` when they failed, and neither
        // was put back.
        assert!(
            lock(&sup.live)
                .get(&id)
                .and_then(|l| l.cancel.as_ref())
                .is_none(),
            "a draining node started an agent for a run it was handing away"
        );
    }

    #[test]
    fn a_departure_is_not_downgraded_by_a_person_asking_afterwards() {
        // `offload checkpoint` and a drain arm the same flag, and until ADR-0042 that was all
        // it held: a bool. It now holds *who*, because the run each of them leaves behind means
        // opposite things — one waits for a person, the other for a bid round — and this moment,
        // several minutes and one turn before the release, is the only one that knows.
        //
        // Which leaves an ordering to decide, and it is reachable: a drain waits up to five
        // minutes, and somebody can type `offload checkpoint` inside that window. The node is
        // leaving either way, so the departure is the fact that survives. Getting it backwards
        // writes `Parked` on a run nobody will come back for, which is precisely the stranding
        // this field exists to stop.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let (drained_first, run) = dormant_run(&sup, &store);
        let _cancel = sup.register(drained_first, run.epoch).expect("registered");
        sup.request_checkpoint(drained_first, GivenUp::LetGo)
            .expect("the drain asks");
        sup.request_checkpoint(drained_first, GivenUp::Parked)
            .expect("and a person asks a moment later");
        assert_eq!(
            sup.checkpoint_requested(drained_first),
            Some(GivenUp::LetGo),
            "the node is still leaving, whatever else was typed at it"
        );

        // And the other order, which is the one that actually happens: a person parks a run,
        // then the laptop is drained before the turn ends.
        let (parked_first, run) = dormant_run(&sup, &store);
        let _cancel = sup.register(parked_first, run.epoch).expect("registered");
        sup.request_checkpoint(parked_first, GivenUp::Parked)
            .expect("a person asks");
        sup.request_checkpoint(parked_first, GivenUp::LetGo)
            .expect("then the drain does");
        assert_eq!(sup.checkpoint_requested(parked_first), Some(GivenUp::LetGo));

        // A person alone still means a person, which is the case the whole distinction is for.
        let (only_parked, run) = dormant_run(&sup, &store);
        let _cancel = sup.register(only_parked, run.epoch).expect("registered");
        sup.request_checkpoint(only_parked, GivenUp::Parked)
            .expect("request");
        assert_eq!(sup.checkpoint_requested(only_parked), Some(GivenUp::Parked));
    }

    #[tokio::test]
    async fn a_checkpoint_cannot_be_requested_of_a_run_nobody_is_running() {
        // There is no turn boundary coming, so the request would wait forever. Saying so
        // beats accepting it and never honouring it.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let id = sup
            .submit(submission(), Capacity::runs(4))
            .await
            .expect("submit");

        let err = supervisor_with(store)
            .request_checkpoint(id, GivenUp::Parked)
            .expect_err("a restarted daemon hosts nothing");
        assert!(err.to_string().contains("not running here"), "{err}");
    }

    #[test]
    fn the_cadence_is_per_turn_unless_configured_otherwise() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        assert!(sup.cadence_due(1) && sup.cadence_due(2), "one per turn");

        let every_third = Supervisor::new(
            Arc::new(Config {
                checkpoint: crate::config::CheckpointConfig {
                    every_turns: 3,
                    ..crate::config::CheckpointConfig::default()
                },
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger();
        assert!(!every_third.cadence_due(2));
        assert!(every_third.cadence_due(3) && every_third.cadence_due(6));

        let never = Supervisor::new(
            Arc::new(Config {
                checkpoint: crate::config::CheckpointConfig {
                    every_turns: 0,
                    ..crate::config::CheckpointConfig::default()
                },
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store,
        )
        .with_private_ledger();
        assert!(
            !never.cadence_due(1) && !never.cadence_due(10),
            "zero disables automatic checkpoints entirely"
        );
    }

    #[tokio::test]
    async fn a_checkpoint_round_trips_a_worktree_through_the_blob_store() {
        // The phase 2 claim, end to end and without an agent: what the run had at a turn
        // boundary — a commit, an edit, and a file git had never heard of — comes back
        // after the worktree is destroyed.
        let scratch = std::env::temp_dir().join(format!("offload-cp-sup-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([42; 16]);
        let workspace = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        // A run in the state a turn boundary finds it in: assigned to us, started, live.
        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        // What the agent did: a commit, an uncommitted edit, and a brand-new file.
        commit(&workspace.path, "feature.rs", "fn feature() {}\n").await;
        std::fs::write(workspace.path.join("README.md"), "edited\n").expect("edit");
        std::fs::write(workspace.path.join("wip.rs"), "fn half(\n").expect("write");

        let session = "11111111-2222-3333-4444-555555555555";
        transcript::install(&sup.home, &workspace.path, session, &conversation(1))
            .expect("transcript");

        sup.checkpoint(run_id, &workspace, session, "2.1.220", 1, None)
            .await
            .expect("checkpoint");

        let stored = store
            .load_run(run_id)
            .expect("load")
            .expect("present")
            .checkpoint
            .expect("a checkpoint was recorded");
        assert_eq!(stored.session_id.as_deref(), Some(session));
        assert_eq!(
            stored.base_commit, workspace.base_commit,
            "without the base commit a receiving node has nowhere to apply the rest"
        );
        assert!(stored.bundle.is_some(), "the commit travels");
        assert!(stored.patch.is_some(), "and so does the uncommitted work");

        // The machine loses the worktree entirely — mirror and blobs are all that is left.
        sup.workspaces
            .hold(run_id)
            .await
            .remove(&source)
            .await
            .expect("remove");
        assert!(!workspace.path.exists());

        let (restored, how) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &stored)
            .await
            .expect("restore");

        assert!(how.how.contains("patch"), "how it came back: {}", how.how);
        assert_eq!(
            restored.base_commit, workspace.base_commit,
            "the run's base survives the rebuild, so the next checkpoint still bundles"
        );
        assert!(
            restored.path.join("feature.rs").is_file(),
            "the committed work"
        );
        assert_eq!(
            std::fs::read_to_string(restored.path.join("README.md")).expect("read"),
            "edited\n",
            "the uncommitted edit"
        );
        assert_eq!(
            std::fs::read_to_string(restored.path.join("wip.rs")).expect("read"),
            "fn half(\n",
            "and the file git never knew about — the part that is only in the patch"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn a_run_that_comes_back_keeps_the_commits_the_other_machine_made() {
        // **The sibling of the test below, and the half it leaves out.** That one gives the
        // returning checkpoint `bundle: None` and puts the far leg's work in an *uncommitted*
        // file, so it walks the patch and never the bundle. An agent that commits — which is
        // what an agent is for — takes the other path, and it was silently dropping the work.
        //
        // `restore_workspace` asked `has_branch`: this node ran leg one, so `offload/run-…` is
        // in its mirror at leg one's tip, so the branch was checked out as the start ref and
        // the bundle was skipped entirely. The comment said the agent's commits "are already
        // here", which is true of a branch this node put there by a *successful* restore and
        // false of one that is simply older than the checkpoint. Measured on two daemons:
        // alpha's six commits and bravo's shared **only the base commit**, and `offload logs`
        // said `re-checked out, patch reapplied` while the bundle sat fetched and unused.
        let scratch = std::env::temp_dir().join(format!("offload-return-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([44; 16]);
        let session = "11111111-2222-3333-4444-555555555555";
        let here = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        // Leg one, here: a commit, then the checkpoint that hands the run over.
        commit(&here.path, "leg_one.rs", "fn one() {}\n").await;
        transcript::install(&sup.home, &here.path, session, &conversation(1)).expect("transcript");
        sup.checkpoint(run_id, &here, session, "2.1.220", 1, Some(GivenUp::Parked))
            .await
            .expect("checkpoint");

        // Leg two, on another machine: it restores leg one and **commits** on top.
        let elsewhere = WorkspaceManager::new(scratch.join("state-b"));
        let mut there = elsewhere
            .hold(run_id)
            .await
            .prepare(&source, Some(&here.base_commit), Some(&here.branch))
            .await
            .expect("prepare elsewhere");
        there.base_commit.clone_from(&here.base_commit);
        let first = store
            .load_run(run_id)
            .expect("load")
            .expect("present")
            .checkpoint
            .expect("checkpointed");
        let one_bundle = first
            .bundle
            .map(|hash| store.get_blob(hash))
            .transpose()
            .expect("bundle blob");
        offload_workspace::checkpoint::restore(&there, one_bundle.as_deref(), None)
            .await
            .expect("restore elsewhere");
        commit(&there.path, "leg_two.rs", "fn two() {}\n").await;
        let captured = offload_workspace::checkpoint::capture(
            &there,
            &offload_workspace::UntrackedPolicy::default(),
        )
        .await
        .expect("capture elsewhere");
        assert!(
            captured.bundle.is_some(),
            "the staging: the far leg committed, so its checkpoint carries a bundle"
        );

        // Back here, with the branch in this node's mirror still at leg one's tip.
        let second = Checkpoint {
            session_id: Some(session.to_string()),
            transcript: store.put_blob(&conversation(2)).expect("transcript blob"),
            bundle: captured
                .bundle
                .map(|b| store.put_blob(&b))
                .transpose()
                .expect("bundle blob"),
            patch: None,
            base_commit: here.base_commit.clone(),
            turns: 2,
            taken_at: now(),
            agent_version: "2.1.220".into(),
            replicas: std::collections::BTreeSet::new(),
        };

        let (workspace, restored) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &second)
            .await
            .expect("restore");
        let how = &restored.how;
        assert!(
            workspace.path.join("leg_one.rs").is_file(),
            "this node's own leg is still there ({how})"
        );
        assert!(
            workspace.path.join("leg_two.rs").is_file(),
            "and so is what the other machine *committed* — the work a stale branch silently \
             dropped ({how})"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn a_branch_ahead_of_the_checkpoint_is_not_reset_back_onto_it() {
        // **The other direction, and the property the `has_branch` guard was protecting.**
        // Now that the bundle is applied whenever the checkpoint has one, the thing that must
        // not happen is a `reset --hard` onto a capture this checkout has already moved past:
        // a checkpoint is taken at a turn boundary and the agent goes on working, so a branch
        // being *ahead* of the newest checkpoint is the ordinary state of a live run, not an
        // edge. Losing that would trade a silent loss of somebody else's commits for a silent
        // loss of our own, which is not a fix.
        let scratch = std::env::temp_dir().join(format!("offload-ahead-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([45; 16]);
        let session = "11111111-2222-3333-4444-555555555555";
        let here = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        // A capture at turn 1 — and then the agent keeps going, which is what agents do.
        commit(&here.path, "turn_one.rs", "fn one() {}\n").await;
        transcript::install(&sup.home, &here.path, session, &conversation(1)).expect("transcript");
        sup.checkpoint(run_id, &here, session, "2.1.220", 1, Some(GivenUp::Parked))
            .await
            .expect("checkpoint");
        let old = store
            .load_run(run_id)
            .expect("load")
            .expect("present")
            .checkpoint
            .expect("checkpointed");
        assert!(old.bundle.is_some(), "the staging: turn 1 committed");
        commit(&here.path, "turn_two.rs", "fn two() {}\n").await;

        // The worktree goes; the branch, two commits deep, does not. Restoring the *older*
        // checkpoint must not walk it back.
        sup.workspaces
            .hold(run_id)
            .await
            .remove(&source)
            .await
            .expect("remove");

        let (workspace, restored) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &old)
            .await
            .expect("restore");
        let how = &restored.how;
        assert!(
            workspace.path.join("turn_one.rs").is_file(),
            "the checkpoint's own commit ({how})"
        );
        assert!(
            workspace.path.join("turn_two.rs").is_file(),
            "and the turn taken after it, which a reset onto the bundle would have erased \
             ({how})"
        );
        assert!(
            how.contains("already held"),
            "and it says the checkout was left alone rather than claiming a rebuild ({how})"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn a_worktree_from_an_earlier_leg_is_not_mistaken_for_the_current_one() {
        // A run that comes *back*. This node ran it, checkpointed at turn 1 and handed it
        // over; it did more turns somewhere else; now it is here again. Nothing removes a
        // worktree when a run leaves — `cleanup` is deliberately manual and only for
        // terminal runs — so the checkout from the first leg is still on disk, and so is the
        // transcript beside it. Adopting either one resumes the agent into a conversation and
        // a filesystem from before everything the other machine did, silently, while the log
        // says "adopted in place".
        let scratch = std::env::temp_dir().join(format!("offload-legs-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([43; 16]);
        let session = "11111111-2222-3333-4444-555555555555";
        let here = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        // Leg one: one turn's work, then the checkpoint that hands the run over.
        std::fs::write(here.path.join("leg_one.rs"), "fn one() {}\n").expect("write");
        transcript::install(&sup.home, &here.path, session, &conversation(1)).expect("transcript");
        sup.checkpoint(run_id, &here, session, "2.1.220", 1, Some(GivenUp::Parked))
            .await
            .expect("checkpoint");
        let first = store
            .load_run(run_id)
            .expect("load")
            .expect("present")
            .checkpoint
            .expect("checkpointed");

        // Leg two, on another machine: its own state directory, so its own worktree for the
        // same run. It restores what we captured, does more work, and checkpoints again.
        let elsewhere = WorkspaceManager::new(scratch.join("state-b"));
        let mut there = elsewhere
            .hold(run_id)
            .await
            .prepare(&source, Some(&here.base_commit), Some(&here.branch))
            .await
            .expect("prepare elsewhere");
        there.base_commit.clone_from(&here.base_commit);
        let patch = first
            .patch
            .map(|hash| store.get_blob(hash))
            .transpose()
            .expect("patch blob");
        offload_workspace::checkpoint::restore(&there, None, patch.as_deref())
            .await
            .expect("restore elsewhere");
        std::fs::write(there.path.join("leg_two.rs"), "fn two() {}\n").expect("write");
        let captured = offload_workspace::checkpoint::capture(
            &there,
            &offload_workspace::UntrackedPolicy::default(),
        )
        .await
        .expect("capture elsewhere");

        // Its blobs arrive here the way replication puts them here, and its checkpoint the
        // way gossip does: at turn 2, which is a turn this node's checkout never saw.
        let second = Checkpoint {
            session_id: Some(session.to_string()),
            transcript: store.put_blob(&conversation(2)).expect("transcript blob"),
            bundle: None,
            patch: captured
                .patch
                .map(|p| store.put_blob(&p))
                .transpose()
                .expect("patch blob"),
            base_commit: here.base_commit.clone(),
            turns: 2,
            taken_at: now(),
            agent_version: "2.1.220".into(),
            replicas: std::collections::BTreeSet::new(),
        };

        let (workspace, restored) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &second)
            .await
            .expect("restore");
        let how = &restored.how;
        assert!(
            workspace.path.join("leg_one.rs").is_file(),
            "the first leg's work is still there ({how})"
        );
        assert!(
            workspace.path.join("leg_two.rs").is_file(),
            "and so is what the other machine did — the work a stale checkout silently \
             drops ({how})"
        );
        assert!(
            how.contains("kept at"),
            "and the checkout that was moved aside is named, so it can be found ({how})"
        );

        sup.install_transcript(
            &workspace,
            &SessionId(session.to_string()),
            &second,
            &restored,
        )
        .expect("transcript");
        let conversation = std::fs::read_to_string(transcript::expected_path(
            &sup.home,
            &workspace.path,
            session,
        ))
        .expect("read transcript");
        assert!(
            conversation.contains("msg_2"),
            "the agent resumes the conversation as it stands, not as this node last saw it: \
             {conversation}"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn an_earlier_leg_that_committed_everything_is_removed_and_the_door_is_recorded() {
        // The third answer beside the two above, and the one nothing gave for two phases. This
        // run also came back — but leg one **committed** its work before handing over, so the
        // checkout here holds nothing the mirror does not. `supersede` renamed it anyway,
        // unconditionally, and the rename deletes the copy's `.git` file: from that moment
        // nothing could tell this directory from the one holding somebody's only mid-turn edit,
        // and nothing in the product ever removed either. Measured at the crate level — three
        // legs of one run left three full copies of the tree.
        //
        // Two things are asserted, and the second is the point of the row: the copy is gone, and
        // the audit log says which door took it. A `tracing::info!` on an unattended machine is
        // what the `Reclaimed` variant already exists to replace.
        let scratch =
            std::env::temp_dir().join(format!("offload-redundant-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([45; 16]);
        let session = "33333333-4444-5555-6666-777777777777";
        let here = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        // The staging, and the only difference from the test above: leg one **commits**. Its
        // work is on `offload/run-…` in the mirror, which `remove` keeps on purpose.
        std::fs::write(here.path.join("leg_one.rs"), "fn one() {}\n").expect("write");
        for arg in [
            ["config", "user.email", "t@offload.local"],
            ["config", "user.name", "Offload Test"],
        ] {
            tokio::process::Command::new("git")
                .arg("-C")
                .arg(&here.path)
                .args(arg)
                .output()
                .await
                .expect("git config");
        }
        for arg in [vec!["add", "."], vec!["commit", "--quiet", "-m", "leg one"]] {
            tokio::process::Command::new("git")
                .arg("-C")
                .arg(&here.path)
                .args(&arg)
                .output()
                .await
                .expect("git");
        }
        transcript::install(&sup.home, &here.path, session, &conversation(1)).expect("transcript");
        sup.workspaces.note_turn(run_id, 1);

        // A checkpoint from turn 2, the way gossip delivers one from the machine that has the
        // run now. Nothing of leg one's is in it — this is the case where the checkout on this
        // disk is behind and its contents are already elsewhere.
        let second = Checkpoint {
            session_id: Some(session.to_string()),
            transcript: store.put_blob(&conversation(2)).expect("transcript blob"),
            bundle: None,
            patch: None,
            base_commit: here.base_commit.clone(),
            turns: 2,
            taken_at: now(),
            agent_version: "2.1.220".into(),
            replicas: std::collections::BTreeSet::new(),
        };

        let (_workspace, restored) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &second)
            .await
            .expect("restore");
        let how = &restored.how;
        assert!(
            how.contains("nothing uncommitted"),
            "the sentence has to say which of the two happened, or a person looking for a file \
             learns nothing from either ({how})"
        );
        assert!(
            !how.contains("kept at"),
            "and must not claim a rescue it did not make ({how})"
        );

        let strays: Vec<_> = std::fs::read_dir(sup.workspaces.worktrees_dir())
            .expect("read worktrees dir")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .filter(|name| name.contains("superseded"))
            .collect();
        assert!(
            strays.is_empty(),
            "no copy of it was kept for ever, which is what nothing ever removed: {strays:?}"
        );

        // The row, and the door on it. Without this the removal is a log line on a machine
        // nobody is logged into — the whole argument for `AuditEvent::Reclaimed` existing.
        let door = store
            .audit(Some(run_id), 100)
            .expect("audit")
            .into_iter()
            .find_map(|entry| match entry.kind {
                offload_core::AuditEvent::Reclaimed { why, .. } => Some(why),
                _ => None,
            });
        assert_eq!(
            door,
            Some(offload_core::Reclamation::Redundant),
            "the third door: superseded, and holding nothing the mirror did not have"
        );
        assert!(
            !store
                .audit(Some(run_id), 100)
                .expect("audit")
                .iter()
                .any(|entry| matches!(entry.kind, offload_core::AuditEvent::Rescued { .. })),
            "and nothing was rescued, so nothing may say it was"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn a_redundant_checkout_and_commits_the_bundle_moves_past_are_two_rows_for_one_run() {
        // **The sweep over the test above's own fixture.** It sets `bundle: None`, so the
        // redundant-checkout arm is walked with nothing for `restore` to reset onto — and the
        // other rescue on this same path, the one that names commits a `reset --hard` moves the
        // branch off, can therefore never fire in it. That is the trap the checkpoints pitfall
        // file already names once: **when a fixture sets a field to `None`, ask what that field
        // being `Some` would have done.** Here it produces the case where both happen to one run
        // in one resume, which is what an operator reading `offload audit` actually meets.
        //
        // The staging: leg one commits (so its checkout is redundant — every byte on the run
        // branch) and the returning checkpoint's bundle carries a **different** commit on the
        // same base, so the branch this node re-checks out has diverged from it. One is a
        // `Reclaimed { Redundant }` row, the other a `Rescued { Commits }`, and neither may
        // swallow the other.
        let scratch = std::env::temp_dir().join(format!("offload-bothrows-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([46; 16]);
        let session = "44444444-5555-6666-7777-888888888888";

        // A helper, because both legs need an identity and a commit and the difference between
        // them is one filename.
        async fn commit(at: &std::path::Path, file: &str) {
            for args in [
                vec!["config", "user.email", "t@offload.local"],
                vec!["config", "user.name", "Offload Test"],
            ] {
                tokio::process::Command::new("git")
                    .arg("-C")
                    .arg(at)
                    .args(&args)
                    .output()
                    .await
                    .expect("git config");
            }
            std::fs::write(at.join(file), "fn work() {}\n").expect("write");
            for args in [vec!["add", "."], vec!["commit", "--quiet", "-m", file]] {
                tokio::process::Command::new("git")
                    .arg("-C")
                    .arg(at)
                    .args(&args)
                    .output()
                    .await
                    .expect("git");
            }
        }

        let here = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        commit(&here.path, "ours.rs").await;
        transcript::install(&sup.home, &here.path, session, &conversation(1)).expect("transcript");
        sup.workspaces.note_turn(run_id, 1);

        // The other machine's leg, on its own state directory: the same base, a different
        // commit. That is divergence, which is what makes the reset destructive and the
        // left-behind ref worth writing.
        let elsewhere = WorkspaceManager::new(scratch.join("state-b"));
        let mut there = elsewhere
            .hold(run_id)
            .await
            .prepare(&source, Some(&here.base_commit), None)
            .await
            .expect("prepare elsewhere");
        there.base_commit.clone_from(&here.base_commit);
        commit(&there.path, "theirs.rs").await;
        let captured = offload_workspace::checkpoint::capture(
            &there,
            &offload_workspace::UntrackedPolicy::default(),
        )
        .await
        .expect("capture elsewhere");
        let bundle = captured
            .bundle
            .expect("the far leg committed, so there is a bundle");

        let second = Checkpoint {
            session_id: Some(session.to_string()),
            transcript: store.put_blob(&conversation(2)).expect("transcript blob"),
            bundle: Some(store.put_blob(&bundle).expect("bundle blob")),
            patch: None,
            base_commit: here.base_commit.clone(),
            turns: 2,
            taken_at: now(),
            agent_version: "2.1.220".into(),
            replicas: std::collections::BTreeSet::new(),
        };

        let (workspace, restored) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &second)
            .await
            .expect("restore");
        let how = &restored.how;
        assert!(
            workspace.path.join("theirs.rs").is_file(),
            "the far leg's commit arrived, which is what the bundle is for ({how})"
        );
        assert!(
            how.contains("nothing uncommitted"),
            "the redundant checkout is still reported as removed ({how})"
        );
        assert!(
            how.contains("kept at") || how.contains("only the reflog"),
            "and the commits the reset moved past are still accounted for — the half this \
             fixture's `bundle: None` sibling cannot reach ({how})"
        );

        let kinds: Vec<_> = store
            .audit(Some(run_id), 100)
            .expect("audit")
            .into_iter()
            .map(|entry| entry.kind)
            .collect();
        assert!(
            kinds.iter().any(|k| matches!(
                k,
                offload_core::AuditEvent::Reclaimed {
                    why: offload_core::Reclamation::Redundant,
                    ..
                }
            )),
            "the checkout it removed: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| matches!(
                k,
                offload_core::AuditEvent::Rescued {
                    what: offload_core::Rescue::Commits { named: true },
                    ..
                }
            )),
            "and the commits it kept, which are the only copy of that leg: {kinds:?}"
        );
        assert!(
            !kinds.iter().any(|k| matches!(
                k,
                offload_core::AuditEvent::Rescued {
                    what: offload_core::Rescue::Checkout,
                    ..
                }
            )),
            "and nothing claims a checkout was kept, because none was: {kinds:?}"
        );

        // The point of the ref, rather than of the row: the abandoned commit is reachable by
        // name in the mirror, so it outlives the reflog that would otherwise be all there is.
        let refs = tokio::process::Command::new("git")
            .arg("-C")
            .arg(sup.workspaces.mirror_path(&source))
            .args([
                "for-each-ref",
                "--format=%(refname)",
                "refs/offload/left-behind",
            ])
            .output()
            .await
            .expect("for-each-ref");
        let refs = String::from_utf8_lossy(&refs.stdout);
        assert!(
            refs.contains(&format!("refs/offload/left-behind/{run_id}/")),
            "named under this run, so it is findable without archaeology: {refs:?}"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn the_checkout_that_produced_the_checkpoint_is_adopted_untouched() {
        // The other half of the same decision, and the reason it is a comparison rather than
        // a rule. This run never left: the checkout here is the one that produced the
        // checkpoint and has done work since, and the transcript beside it is this
        // conversation. Rebuilding from blobs would replace both with older copies.
        let scratch = std::env::temp_dir().join(format!("offload-adopt-{}", std::process::id()));
        std::fs::remove_dir_all(&scratch).ok();
        std::fs::create_dir_all(&scratch).expect("mkdir");

        let source = git_repo(&scratch.join("src")).await;
        let store = Store::open(&scratch.join("store")).expect("store");
        let sup = Supervisor::new(
            Arc::new(Config {
                state_dir: scratch.join("state"),
                ..Config::default()
            }),
            NodeId::from_bytes([1; 32]),
            store.clone(),
        )
        .with_private_ledger()
        .with_home(scratch.join("home"));

        let run_id = RunId::from_bytes([44; 16]);
        let session = "22222222-3333-4444-5555-666666666666";
        let here = sup
            .workspaces
            .hold(run_id)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        let mut run = Run::new(run_id, spec(&source), sup.node_id, now());
        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        store.save_run(&run).expect("save");
        let _cancel = sup.register(run_id, epoch).expect("registered");

        std::fs::write(here.path.join("captured.rs"), "fn captured() {}\n").expect("write");
        transcript::install(&sup.home, &here.path, session, &conversation(1)).expect("transcript");
        sup.checkpoint(run_id, &here, session, "2.1.220", 1, None)
            .await
            .expect("checkpoint");
        let checkpoint = store
            .load_run(run_id)
            .expect("load")
            .expect("present")
            .checkpoint
            .expect("checkpointed");

        // Then the agent kept going and the process died — a crash, not a migration. This is
        // the only copy of either of these.
        std::fs::write(here.path.join("since.rs"), "fn mid_turn(\n").expect("write");
        transcript::install(&sup.home, &here.path, session, &conversation(2)).expect("transcript");

        let (workspace, restored) = sup
            .restore_workspace(&sup.workspaces.hold(run_id).await, &source, &checkpoint)
            .await
            .expect("restore");
        assert_eq!(workspace.path, here.path);
        assert_eq!(restored.how, "adopted in place");
        assert!(
            workspace.path.join("since.rs").is_file(),
            "work done after the checkpoint is not rolled back to it"
        );

        sup.install_transcript(
            &workspace,
            &SessionId(session.to_string()),
            &checkpoint,
            &restored,
        )
        .expect("transcript");
        let conversation = std::fs::read_to_string(transcript::expected_path(
            &sup.home,
            &workspace.path,
            session,
        ))
        .expect("read transcript");
        assert!(
            conversation.contains("msg_2"),
            "nor is the conversation: {conversation}"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn turn_numbers_continue_the_run_rather_than_the_process() {
        // Found live: a resumed run reported "turn 1" straight after a checkpoint taken at
        // turn 6. Turn counts are what `ps` shows, what the cadence divides, and what a
        // checkpoint claims to hold — all three go wrong if they reset per process.
        let boundary = accumulate_turns(AgentEvent::TurnBoundary { turn: 1 }, 6);
        assert_eq!(boundary, AgentEvent::TurnBoundary { turn: 7 });

        let outcome = offload_agent::Outcome {
            success: true,
            turns: 3,
            stop_reason: None,
            result: None,
            duration_ms: None,
            cost_micro_usd: None,
            permission_denials: 0,
        };
        let AgentEvent::Finished(shifted) =
            accumulate_turns(AgentEvent::Finished(outcome.clone()), 6)
        else {
            panic!("still a finish event");
        };
        assert_eq!(shifted.turns, 9);

        assert_eq!(
            accumulate_turns(AgentEvent::TurnBoundary { turn: 1 }, 0),
            AgentEvent::TurnBoundary { turn: 1 },
            "a fresh run counts from one, untouched"
        );
    }

    #[tokio::test]
    async fn a_full_node_holds_a_run_instead_of_refusing_it() {
        // ADR-0006's accepting-without-starting. The alternative — declining because there is
        // no room — leaves the run pending with nobody committed to it, which is what the
        // overnight case dies of: the laptop that submitted it is about to be shut.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (busy, _) = dormant_run(&sup, &store);

        // One slot, and it is taken.
        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        let held = run.id;
        sup.take_run(run, Capacity::runs(1))
            .await
            .expect("accepted, not refused");

        let stored = sup.run(held).expect("recorded");
        assert_eq!(stored.state.name(), "assigned", "committed, not started");
        assert_eq!(
            stored.holder(),
            Some(sup.node_id),
            "and the commitment carries a lease, which is what makes it recoverable"
        );
        assert!(
            !lock(&sup.live).contains_key(&held),
            "no agent was started for it"
        );

        // Still no room: the tick leaves it exactly where it is.
        assert!(sup.start_held_runs(Capacity::runs(1)).await.is_empty());
        assert_eq!(sup.run(held).expect("still here").state.name(), "assigned");

        // The slot frees. Nothing tells the supervisor — it notices, which is why this is a
        // poll: the run ahead might equally have been cancelled or migrated away.
        let mut finished = sup.run(busy).expect("the busy one");
        let epoch = finished.epoch;
        finished
            .complete(sup.node_id, epoch, now())
            .expect("complete");
        store.save_run(&finished).expect("save");

        // It goes down the ordinary start path now. Whether the agent then survives is not
        // this test's business — the repo is a path that does not exist — but registering the
        // run as live is `start_run`'s first act, and it is the difference between held and
        // started.
        sup.start_held_runs(Capacity::runs(1)).await;
        assert!(
            lock(&sup.live).contains_key(&held),
            "the held run was started once there was room"
        );
    }

    /// A fleet whose blob fetch takes as long as the test wants it to.
    ///
    /// Not a contrivance. `ensure_blobs` is a network round trip, and it is the *only* `await`
    /// inside `start_run` — so it is exactly the gap the tick's snapshot of held runs has to
    /// survive, and a migration is the ordinary way to open it.
    #[derive(Debug)]
    struct GatedFetch {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl Peers for GatedFetch {
        async fn replicate(&self, _run: RunId, _blobs: Vec<offload_core::BlobHash>) -> Replicated {
            Replicated::NoPeer
        }

        async fn fetch(&self, _blob: offload_core::BlobHash) -> Result<(), String> {
            self.entered.notify_one();
            self.release.notified().await;
            Err("this test's fleet has no blobs".into())
        }
    }

    #[tokio::test]
    async fn a_run_cancelled_while_the_tick_is_starting_another_is_not_started_anyway() {
        // `start_held_runs` snapshots the runs it will start and then works through them, so
        // every copy after the first is as old as everything the runs ahead of it did. One of
        // those things is a blob fetch across the network. `start_run` then wrote the copy it
        // was handed with `save_run`, which is a whole-row write: the cancel that landed in
        // between was not overruled, it was erased, and the run started.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        sup.peers_via(Arc::new(GatedFetch {
            entered: entered.clone(),
            release: release.clone(),
        }));

        // Two runs this node has accepted and not started. Priority orders them, so which one
        // the tick reaches first is decided rather than observed.
        let held = |priority: i32, checkpoint: Option<Checkpoint>| {
            let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
            let mut spec = spec(&RepoSource::parse("/tmp/whatever"));
            spec.priority = priority;
            let mut run = Run::new(id, spec, sup.node_id, now());
            run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
            run.checkpoint = checkpoint;
            store.save_run(&run).expect("save");
            id
        };
        let migrating = held(10, Some(checkpoint_at(4)));
        let queued = held(0, None);

        let ticking = sup.clone();
        let tick = tokio::spawn(async move { ticking.start_held_runs(Capacity::runs(4)).await });

        // The migration is in the middle of fetching its transcript.
        entered.notified().await;
        assert!(
            lock(&sup.live).contains_key(&migrating),
            "the first of the two is the one being started"
        );

        // Somebody cancels the *other* one while it is.
        sup.cancel_run(queued, "operator").await.expect("cancelled");
        assert_eq!(sup.run(queued).expect("here").state.name(), "cancelled");

        release.notify_one();
        tick.await.expect("tick");

        assert_eq!(
            sup.run(queued).expect("still here").state.name(),
            "cancelled",
            "the tick was holding a copy from before the cancel and wrote it back over it"
        );
        assert!(
            !lock(&sup.live).contains_key(&queued),
            "and then started an agent for a run somebody had cancelled"
        );
    }

    #[tokio::test]
    async fn a_second_start_for_a_run_already_going_here_starts_no_second_agent() {
        // One grant, two workspaces — the entry `docs/pitfalls/fencing-and-epochs.md` carried as
        // open. `register` *replaced* the running leg's entry, so nothing anywhere refused a
        // second `start_run` for a run this node was already running: `hold_here` waves it
        // through, because the row says this node holds the run under a lease of its own, which
        // is precisely what the leg already going put there. Both legs reach `drive`, both build
        // a workspace, and `git worktree add` is a read-decide-write of its own — 17 of 200
        // forced races failing with `cannot force update the branch … used by worktree at
        // <the same path>`, the sentence in the session fifty-two logs.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        sup.peers_via(Arc::new(GatedFetch {
            entered: entered.clone(),
            release: release.clone(),
        }));

        // A migration, which is the ordinary way to hold a start open: the checkpoint's blobs
        // are fetched *after* the registration, which is the state the second caller has to meet.
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut run = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.checkpoint = Some(checkpoint_at(4));
        store.save_run(&run).expect("save");

        let starting = sup.clone();
        let first = tokio::spawn(async move { starting.start_run(run).await });
        entered.notified().await;
        let going = lock(&sup.live)
            .get(&id)
            .expect("the first leg is registered")
            .epoch;

        // The second caller, with a copy it was handed: the tick's snapshot, a grant re-offered,
        // `take_run`'s take-back after a round nobody won. Which of them it is does not matter
        // here, and that is the point of refusing in the writer rather than at each of them.
        let again = sup.run(id).expect("recorded");
        // Bounded, because the shape of the failure is *not* an error: without the refusal the
        // second leg registers over the first and walks on into the blob fetch, where it waits
        // on a gate only the first leg's release will open. A hang is what a missing guard looks
        // like from here, so the timeout is the assertion.
        let err = tokio::time::timeout(std::time::Duration::from_secs(5), sup.start_run(again))
            .await
            .expect("the second start went past the registration and into the blob fetch")
            .expect_err("a second agent");
        assert!(
            matches!(err, SubmitError::AlreadyGoing { .. }),
            "refused as its own fact, not as a failed start: {err}"
        );

        // And the leg that *is* going still owns the registration. Losing it is not untidy: the
        // cancel sender goes with the entry, so the running agent becomes one nothing can stop,
        // and `cancel_run` answers "its agent was stopped" about the leg that never started.
        assert_eq!(
            lock(&sup.live).get(&id).expect("still registered").epoch,
            going,
            "the running leg's registration was replaced"
        );

        release.notify_one();
        first
            .await
            .expect("joined")
            .expect_err("no blobs in this fleet");
    }

    #[tokio::test]
    async fn a_grant_still_beats_this_node_s_own_memory_of_the_leg_before_it() {
        // The other direction, and the reason `start_run` does not simply read the row and
        // ignore what it was handed. A grant is *news*: the arbiter assigned the run at an
        // epoch the fleet agreed on, and this node's row is whatever gossip last told it about
        // the leg that ran before ours — older by construction, and carrying no checkpoint,
        // which is the whole of what makes a migration a migration rather than a restart.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let previous = NodeId::from_bytes([7; 32]);

        // What this node remembers: the run on the machine it came from.
        let id = RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes());
        let mut memory = Run::new(
            id,
            spec(&RepoSource::parse("/tmp/whatever")),
            previous,
            now(),
        );
        let epoch = memory.assign(previous, now(), LEASE_TTL).expect("assign");
        memory.started(previous, epoch, now()).expect("start");
        store.save_run(&memory).expect("save");

        // What arrives: the same run, granted here, one epoch on, with the conversation.
        let mut granted = memory.clone();
        granted.orphan(now()).expect("the holder went quiet");
        let granted_epoch = granted
            .assign(sup.node_id, now(), LEASE_TTL)
            .expect("assign");
        granted.checkpoint = Some(Checkpoint {
            transcript: store.put_blob(b"the conversation so far").expect("blob"),
            ..checkpoint_at(6)
        });
        assert!(granted_epoch > epoch, "a grant spends a token");

        sup.take_run(granted, Capacity::runs(1))
            .await
            .expect("accepted");

        let stored = sup.run(id).expect("recorded");
        assert_eq!(
            stored.epoch, granted_epoch,
            "at the arbiter's number, not ours"
        );
        assert_eq!(stored.holder(), Some(sup.node_id));
        assert_eq!(
            stored.checkpoint.map(|c| c.turns),
            Some(6),
            "and carrying the checkpoint the grant brought, which the row here never had"
        );
        assert!(
            lock(&sup.live).contains_key(&id),
            "and it started, rather than being refused by this node's own stale copy"
        );
    }

    #[tokio::test]
    async fn a_start_that_gives_up_before_the_agent_leaves_nothing_holding_the_slot() {
        // `start_run` registers the run as live before it fetches anything, because the
        // registration is what a cancel arriving mid-start has to find. Everything between
        // that and `launch` returns early on failure, and `launch` is what installs the
        // handler that would have cleaned up — so a migration whose transcript cannot be
        // fetched left an entry in `live` that nothing ever removes.
        //
        // Two consequences, and the second is the one that makes the log line a lie.
        // `held_but_not_started` filters on `!live.contains_key`, so the run never comes back
        // round; and `started_excluding` counts live-or-`Running` for a run this node holds,
        // so the slot stays occupied by an agent that was never started.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        // A migration: held here, with a checkpoint whose transcript is not on this machine.
        // No fleet is configured, so `ensure_blobs` gives up rather than fetching — the same
        // early return as a fetch that fails, and the one this node can reach on its own.
        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.checkpoint = Some(checkpoint_at(4));
        let id = run.id;
        store.save_run(&run).expect("save");

        assert!(
            sup.start_held_runs(Capacity::runs(4)).await.is_empty(),
            "it could not be started"
        );

        assert_eq!(
            sup.run(id).expect("still here").state.name(),
            "assigned",
            "still held, which is what makes the next tick the retry the log promises"
        );
        assert!(
            !lock(&sup.live).contains_key(&id),
            "and nothing is registered for an agent that was never launched"
        );
        assert_eq!(
            sup.held_but_not_started().len(),
            1,
            "so the tick sees it again; `could not start it yet` has to be true of something"
        );
        // The other half of that sentence, asked the way the tick's log asks it: this failure
        // really is the retryable kind, and the filter is what says so rather than the error.
        assert!(sup.will_try_again(id));
    }

    #[tokio::test]
    async fn a_revoked_node_stops_the_agents_it_is_running_and_takes_no_more_work() {
        // ADR-0044. The fleet evicts the node and moves the run; the node, which can hear
        // nobody, ran its old leg to the end — two agents on one repository, measured at turn 31
        // against turn 15. The ordinary supersede path cannot help: `record_run` fires on the new
        // holder's record arriving by gossip, and gossip is exactly what a revocation ends.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);
        let mut cancel_rx = sup.register(id, run.epoch).expect("registered");

        assert!(!sup.is_revoked());
        assert!(!sup.is_draining(), "the control: it would take work now");

        let stopped = sup.stand_down().await.expect("the first pass acts");
        assert_eq!(stopped, vec![id]);
        assert_eq!(
            cancel_rx.recv().await,
            Some(Halt::Revoked),
            "the agent is asked to stop, and told why"
        );
        assert!(sup.is_revoked());
        assert!(
            sup.is_draining(),
            "every door that already asks about a drain has to be shut here too"
        );

        // Idempotent, because it is driven by a poll on a standing condition rather than by an
        // edge: this runs once a second for as long as the daemon is up.
        assert!(
            sup.stand_down().await.is_none(),
            "a second pass has nothing to say and nothing to stop"
        );
    }

    #[tokio::test]
    async fn a_revoked_node_does_not_pick_its_failed_runs_back_up() {
        // The path `stop_accepting` never reached, one more time (ADR-0034 §1's lesson, ADR-0044's
        // case). Auto-resume asks neither the bid nor the grant, so shutting those two doors on a
        // revoked node leaves the one that spawns agents without being asked.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, mut run) = dormant_run(&sup, &store);
        let epoch = run.epoch;
        run.checkpoint = Some(checkpoint_at(4));
        run.fail(sup.node_id, epoch, "the agent died".to_string(), now())
            .expect("fail");
        store.save_run(&run).expect("save");
        let policy = offload_core::RecoveryPolicy::default();
        let seen = offload_core::Circumstances {
            attendance: Some(offload_core::Attendance::Unattended),
            standing: offload_core::Standing::Ours,
            departing: false,
            revoked: false,
            hosting: offload_core::Hosting::Allowed,
            alone: false,
            turns: 0,
            resumes: 0,
        };
        let later = now() + policy.backoff_per_resume + offload_core::Millis(1);
        assert!(
            matches!(
                offload_core::decide_recovery(&run, later, &policy, seen),
                Some(offload_core::Recovery::Resume { .. })
            ),
            "the control: a member picks this one back up"
        );

        sup.stand_down().await.expect("stood down");
        let seen = offload_core::Circumstances {
            revoked: sup.is_revoked(),
            departing: sup.is_draining(),
            ..seen
        };
        assert_eq!(
            offload_core::decide_recovery(&run, later, &policy, seen),
            Some(offload_core::Recovery::Escalate(
                offload_core::Escalation::NodeIsRevoked
            )),
            "and it is the revocation that is said, not the drain the same call also set"
        );
        assert_eq!(id, run.id);
    }

    #[tokio::test]
    async fn a_cancel_that_lands_while_the_start_is_failing_is_not_dropped_with_it() {
        // The half of giving the registration back that is not bookkeeping. `cancel_run` finds
        // the registration, takes the halt channel, signals, and answers "its agent was
        // stopped" — writing **nothing**, because the writer is `drive`. If the start then
        // fails, `drive` never runs: hand the registration back and the halt goes with it, the
        // row stays `assigned`, and the next tick starts a run somebody was told had stopped.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        sup.peers_via(Arc::new(GatedFetch {
            entered: entered.clone(),
            release: release.clone(),
        }));

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.checkpoint = Some(checkpoint_at(4));
        let id = run.id;
        store.save_run(&run).expect("save");

        let ticking = sup.clone();
        let tick = tokio::spawn(async move { ticking.start_held_runs(Capacity::runs(4)).await });

        // Mid-fetch: registered, so the cancel is answered by the agent path.
        entered.notified().await;
        assert_eq!(
            sup.cancel_run(id, "operator").await.expect("cancelled"),
            "its agent was stopped",
            "which is the answer that writes nothing and leaves the writing to `drive`"
        );

        // And then the fetch fails, so `drive` is never reached.
        release.notify_one();
        tick.await.expect("tick");

        assert_eq!(
            sup.run(id).expect("still here").state.name(),
            "cancelled",
            "the halt outlived the start that was carrying it"
        );
        assert!(
            sup.held_but_not_started().is_empty(),
            "and nothing is going to start it again"
        );
        assert_eq!(
            store.load_stats(id).expect("stats").workspace,
            NEVER_STARTED,
        );
    }

    #[tokio::test]
    async fn a_run_that_moved_on_while_it_waited_says_so_rather_than_naming_a_state() {
        // The tick's third refusal, and the last one that was a state machine talking to an
        // operator. A run migrates away while the tick works through the one ahead of it: the
        // row is now `Running` on somebody else, so `assign` refuses it and the message was
        // `agent: cannot assign a run that is running`. As permanent as the terminal case and
        // worse to read. `LostTheRun` is the variant that already means this.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let other = NodeId::from_bytes([7; 32]);

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        // What the tick is holding: the copy from before the handover.
        let snapshot = run.clone();

        let mut moved = run.clone();
        moved.orphan(now()).expect("the holder went quiet");
        let theirs = moved.assign(other, now(), LEASE_TTL).expect("reassign");
        moved.started(other, theirs, now()).expect("start");
        store.save_run(&moved).expect("save");

        let err = sup.start_run(snapshot).await.expect_err("refused");
        assert!(matches!(err, SubmitError::LostTheRun { .. }), "{err}");
        assert!(
            err.to_string().contains("moved on") && err.to_string().contains(&other.short()),
            "and it names where it went: {err}"
        );
        // And the tick's other question is answered by the filter, not by the error: nothing
        // here holds this run any more, so nothing comes back to it.
        assert!(!sup.will_try_again(moved.id));
    }

    #[tokio::test]
    async fn an_orphaned_run_is_reclaimed_here_rather_than_held_with_no_lease() {
        // `Run::holder` answers `Some(last)` for an `Orphaned` run — it is who *was* holding
        // it, which is the right answer to that question and the wrong basis for this one.
        // Asking it let a run whose lease had been taken away take the already-ours branch:
        // measured, the row stayed `orphaned`, the epoch did not move, the agent was registered
        // anyway, and `drive`'s pre-spawn fence caught it a cold clone later and reported a lost
        // run to somebody who had not lost one. `Orphaned` grants no authority and returns no
        // lease, so the base has to be the lease.
        //
        // Both directions in one test, because they are one rule: the last holder's identity is
        // not authority, so an orphaned run is reclaimed here whoever last held it.
        let store = Store::open_memory().expect("store");
        let other = NodeId::from_bytes([7; 32]);

        for (who, label) in [(None, "ours"), (Some(other), "somebody else's")] {
            let sup = supervisor_with(store.clone());
            let last = who.unwrap_or(sup.node_id);
            let mut run = Run::new(
                RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
                spec(&RepoSource::parse("/tmp/whatever")),
                sup.node_id,
                now(),
            );
            let epoch = run.assign(last, now(), LEASE_TTL).expect("assign");
            run.started(last, epoch, now()).expect("start");
            run.orphan(now()).expect("the holder went quiet");
            let id = run.id;
            store.save_run(&run).expect("save");

            sup.start_run(run).await.expect("started");

            let after = sup.run(id).expect("recorded");
            assert_eq!(after.state.name(), "assigned", "{label}");
            assert_eq!(after.holder(), Some(sup.node_id), "{label}");
            assert!(
                after.epoch > epoch,
                "{label}: a reclaim is an assignment, and an assignment spends a token"
            );
            assert!(lock(&sup.live).contains_key(&id), "{label}");
        }
    }

    #[tokio::test]
    async fn a_finished_run_keeps_its_leg_and_lets_go_of_its_stream() {
        // Nothing removed a `live` entry, and the entry is a `broadcast::channel(256)` whose ring
        // is allocated when it is created — measured at about **37 KB per run**, retained for the
        // daemon's lifetime whether anybody followed the run or not, with the store kept out of
        // the measurement. ADR-0020 makes runs cheap enough for that to matter: a trigger firing
        // every three seconds is 1,200 an hour, on a daemon meant to run on a phone.
        //
        // What the entry is *for* has to survive it: `describes` asks whether a finished run
        // ended here, and this leg's epoch is the only thing that can answer.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            // A path that is not a repository, so the agent never starts and `drive` comes out
            // of the arm that writes the run's last line itself.
            spec(&RepoSource::parse("/tmp/offload-not-a-repo-at-all")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        let id = run.id;
        let epoch = run.epoch;
        store.save_run(&run).expect("save");

        // Somebody following, because that is the case where the ring really does hold events.
        sup.start_run(run).await.expect("started");
        let mut follower = lock(&sup.live)
            .get(&id)
            .and_then(|l| l.live.as_ref())
            .map(broadcast::Sender::subscribe)
            .expect("a live run has a stream");

        for _ in 0..200 {
            if sup.run(id).is_some_and(|r| r.state.is_terminal()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            sup.run(id).expect("here").state.is_terminal(),
            "the run ended one way or another"
        );

        // The follower drains what was sent and then learns the run is over, which is what it
        // was waiting for. Closing the channel is how it finds out.
        let mut closed = false;
        for _ in 0..300 {
            match follower.try_recv() {
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Closed) => {
                    closed = true;
                    break;
                }
                Err(_) => break,
            }
        }
        assert!(closed, "the stream ends rather than staying open for ever");

        let (streaming, leg) = {
            let live = lock(&sup.live);
            let entry = live.get(&id).expect("the leg is still recorded");
            (entry.live.is_some(), entry.epoch)
        };
        assert!(!streaming, "and the ring it was holding is gone");
        assert_eq!(leg, epoch, "which is the part `describes` reads");
        assert!(
            sup.describes(id),
            "so this node may still say what its worktree holds"
        );
    }

    #[tokio::test]
    async fn a_run_that_ended_while_it_waited_is_refused_as_an_ending_not_a_delay() {
        // The classification the tick's log line needs. `start_held_runs` logged every refusal
        // under `could not start it yet`, which is a promise that the next tick will try again
        // — and a terminal run holds no lease, so it never appears in `held_but_not_started`
        // again. Nothing tries anything. This is the ordinary end of the race the test above
        // this pair arranges, so it is a line an operator will meet.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        // What the tick is holding: the copy from before somebody cancelled it.
        let snapshot = run.clone();
        run.cancel(now()).expect("cancel");
        store.save_run(&run).expect("save");

        let err = sup.start_run(snapshot).await.expect_err("refused");
        assert!(
            matches!(
                err,
                SubmitError::Ended {
                    state: "cancelled",
                    ..
                }
            ),
            "{err}"
        );
        // And the sentence is the one it always was: the variant exists so the *caller* can
        // tell a permanent refusal from a temporary one, not to say anything new to a person.
        assert!(
            err.to_string()
                .ends_with("it was cancelled while it waited to start"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_cancelled_run_that_never_started_stops_saying_it_is_waiting_for_a_slot() {
        // The worktree summary is where a held run's refusal is published (ADR-0006's
        // accept-without-starting has nothing else to say about a run with no worktree), and
        // it **gossips** — so a reason left behind after the run ended is the whole fleet's
        // answer. Measured on a daemon: `offload ps` showed `cancelled` beside `waiting for a
        // slot` for as long as the record existed.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (_busy, _) = dormant_run(&sup, &store);

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        let held = run.id;
        sup.take_run(run, Capacity::runs(1))
            .await
            .expect("accepted, not refused");
        assert_eq!(
            store.load_stats(held).expect("stats").workspace,
            "waiting for a slot",
            "which is the right answer while it is true"
        );

        sup.cancel_run(held, "operator").await.expect("cancelled");
        assert_eq!(sup.run(held).expect("here").state.name(), "cancelled");
        assert_eq!(
            store.load_stats(held).expect("stats").workspace,
            NEVER_STARTED,
            "and once it is over, the column says what became of the worktree instead"
        );
    }

    #[test]
    fn changing_a_deadline_says_what_it_does_and_refuses_what_it_cannot() {
        // The consequence ADR-0013 calls the most tempting lie in the design: on a running run
        // this buys nothing anybody can see, because there is no preemption and no making an
        // agent think faster. It changes how the *next* failure is handled — invisible right
        // up until it is the only thing that matters — so the command says so rather than
        // letting somebody believe they have hurried it along.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let id = run.id;
        store.save_run(&run).expect("save");

        let (pending, note) = sup
            .edit_spec(
                id,
                offload_core::SpecEdit::Deadline {
                    at: Some(now() + Millis::from_mins(30)),
                },
            )
            .expect("set");
        assert_eq!(pending.spec_rev, 1);
        assert!(note.contains("offered again"), "{note}");

        let epoch = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        run.spec.deadline = pending.spec.deadline;
        run.spec_rev = 1;
        store.save_run(&run).expect("save");

        let (running, note) = sup
            .edit_spec(
                id,
                offload_core::SpecEdit::Deadline {
                    at: Some(now() + Millis::from_mins(5)),
                },
            )
            .expect("set");
        assert_eq!(running.spec_rev, 2, "the owner counts its own writes");
        assert!(note.contains("nothing speeds an agent up"), "{note}");

        // A priority edit shares the counter, because it shares the owner: one record at one
        // revision carries the spec as it stands, whichever field the last edit touched.
        let (reordered, note) = sup
            .edit_spec(id, offload_core::SpecEdit::Priority { to: 10 })
            .expect("set");
        assert_eq!(reordered.spec_rev, 3);
        assert_eq!(reordered.spec.priority, 10);
        assert_eq!(
            reordered.spec.deadline, running.spec.deadline,
            "and the field it did not touch is untouched"
        );
        assert!(note.contains("nothing is preempted"), "{note}");

        // And a finished run is refused rather than given a revision that changes nothing:
        // there is no decision left for the deadline to be an input to.
        let mut done = reordered;
        done.complete(sup.node_id, done.epoch, now()).expect("done");
        store.save_run(&done).expect("save");
        assert!(sup
            .edit_spec(id, offload_core::SpecEdit::Deadline { at: None })
            .is_err());
        assert!(sup
            .edit_spec(id, offload_core::SpecEdit::Priority { to: 0 })
            .is_err());
    }

    #[tokio::test]
    async fn removing_a_run_whose_checkout_is_elsewhere_says_nothing_about_it() {
        // `offload rm` typed on the wrong machine. A finished run has **no holder** — the lease
        // is gone with the terminal transition — so nothing in the record says which node's disk
        // the checkout is on, and the only honest question this node can ask is of its own.
        //
        // It did not ask. `WorkspaceManager::remove` returns `Ok(())` for a path that is not
        // there (idempotent teardown, from phase 1, and right), so `cleanup` went on to write
        // `workspace: "removed"` into this node's numbers for a worktree on another machine.
        // Those numbers gossip: `gossipable_progress` sends every run this node has stats for,
        // holder or not, and within one leg `RunProgress::absorb` settles a tie on the counters
        // by the author's clock — so a copy carrying a *later* one is ahead and wins everywhere.
        // The fleet then reports a checkout as removed while it is still on the desktop, and the
        // operator is told it worked.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let id = run.id;
        // Held and finished somewhere else, which is the ordinary shape for a run submitted
        // here and placed on another machine.
        let elsewhere = NodeId::from_bytes([9; 32]);
        let epoch = run.assign(elsewhere, now(), LEASE_TTL).expect("assign");
        run.started(elsewhere, epoch, now()).expect("start");
        run.complete(elsewhere, epoch, now()).expect("complete");
        assert_eq!(run.holder(), None, "a finished run names no node");
        store.save_run(&run).expect("save");

        // The holder's last word about its own worktree, merged here the ordinary way.
        let theirs = offload_core::RunProgress {
            turns: 31,
            cost_micro_usd: 5_000,
            workspace: "3 modified, 1 new".into(),
            at: now(),
            by: Some(elsewhere),
            epoch,
            ..offload_core::RunProgress::default()
        };
        sup.record_progress(id, &theirs);

        let outcome = sup.cleanup(id).await.expect("cleanup");
        assert_eq!(
            outcome,
            Removal::NothingHere,
            "there is no checkout here to discard"
        );

        let ours = store.load_stats(id).expect("stats");
        assert_eq!(
            ours.workspace, theirs.workspace,
            "a node that removed nothing has nothing to say about somebody else's disk"
        );
        let mut fleet = theirs.clone();
        fleet.absorb(&ours);
        assert_eq!(
            fleet.workspace, theirs.workspace,
            "and nothing the fleet would believe over the machine the checkout is on"
        );
    }

    #[test]
    fn an_edit_to_a_run_we_are_holding_takes_the_edit_and_nothing_else() {
        // The other end of `ClusterView::merge_spec_edit`, which refuses a peer's record for a
        // run held here and takes the two fields on it that were never ours. That rule was true
        // of the view and false of the store: what came down to be written was the *record*, and
        // the copy it came on is whatever the last gossip tick published — so an edit arriving a
        // second after a checkpoint wrote the run back as it stood before that checkpoint, with
        // its blobs then unreferenced and collectable. The edit is one thing; a record is every
        // field, including the ones this node is the only one entitled to state.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (id, run) = dormant_run(&sup, &store);

        // What a peer holds about this run: true when it was published, and a turn out of date
        // now — plus the operator's edit, which is the half that has to get through.
        let mut theirs = run.clone();
        theirs.spec.deadline = Some(now() + Millis::from_mins(30));
        theirs.spec.priority = 7;
        theirs.spec_rev = 1;

        // Meanwhile, here: a turn boundary.
        let checkpoint = Checkpoint {
            replicas: std::collections::BTreeSet::new(),
            session_id: Some("session".into()),
            transcript: store.put_blob(b"transcript").expect("put"),
            bundle: None,
            patch: None,
            base_commit: "c0ffee".into(),
            turns: 19,
            taken_at: now(),
            agent_version: "v".into(),
        };
        store
            .update_run(id, |run| {
                run.record_checkpoint(sup.node_id, run.epoch, checkpoint.clone())
                    .expect("checkpoint");
            })
            .expect("update")
            .expect("there");

        sup.apply_spec_edit(&theirs).expect("edit");

        let stored = store.load_run(id).expect("load").expect("there");
        assert_eq!(
            stored.checkpoint.map(|c| c.turns),
            Some(19),
            "nineteen turns of work, undone by somebody moving a deadline"
        );
        assert_eq!(stored.spec.deadline, theirs.spec.deadline);
        assert_eq!(stored.spec.priority, 7);
        assert_eq!(stored.spec_rev, 1);

        // And the same edit again is not a second one — the revision is the whole of what
        // decides, in both directions, so an older record settles nothing.
        let mut older = theirs.clone();
        older.spec.priority = 0;
        older.spec_rev = 0;
        sup.apply_spec_edit(&older).expect("edit");
        let stored = store.load_run(id).expect("load").expect("there");
        assert_eq!(stored.spec.priority, 7, "a stale revision may not win");
        assert_eq!(stored.spec_rev, 1);
    }

    #[tokio::test]
    async fn a_run_that_comes_back_to_a_node_that_already_ran_a_leg_of_it_still_starts() {
        // A `live` entry outlives the leg that made it — `release` drops the cancel handle and
        // `close_stream` the channel, and the epoch stays because `describes` has nothing else
        // to answer with. So *an entry exists* and *an agent is going here* have never been the
        // same question, and three readers asked the first one meaning the second.
        //
        // The laptop → desktop → laptop path is all it takes: this node runs a leg, the run
        // goes back to the pool, and it is granted here again while the node is full — which
        // takes `hold_here`, and `hold_here` does not touch `live`. The previous leg's entry is
        // still sitting there, `awaits_a_slot` reads it as already started, and the run stays
        // `Assigned` under a lease it renews for ever with no agent behind it and no tick that
        // will ever look at it again. Nothing fails; `ps` says `assigned`.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let id = run.id;

        // A leg that ran here and ended, left exactly as `drive` and `launch` leave it.
        let first = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        run.started(sup.node_id, first, now()).expect("start");
        store.save_run(&run).expect("save");
        let _rx = sup.register(id, first).expect("registered");
        sup.release(id);
        sup.close_stream(id);
        assert!(
            lock(&sup.live).contains_key(&id),
            "the entry stays on purpose; this test is about what reads it"
        );

        // The run goes back to the pool, and comes back granted here at a fresh number while
        // the node is full — so it is a commitment (ADR-0006), not a start.
        run.checkpointed(sup.node_id, first, checkpoint_at(3), GivenUp::Parked, now())
            .expect("checkpoint");
        store.save_run(&run).expect("save");
        let granted = run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        assert!(granted > first, "a grant spends a token");
        sup.take_run(run, Capacity::runs(0))
            .await
            .expect("accepted");

        assert_eq!(
            sup.held_but_not_started().len(),
            1,
            "the tick has to see a commitment made after the last leg ended"
        );
        assert!(
            sup.will_try_again(id),
            "and say so, since the two read one predicate"
        );

        // The second consequence, which is the machine's own capacity: a commitment counted as
        // a running agent is the deadlock two_commitments_on_a_one_slot_node_do_not_block_each_
        // other exists to prevent, arriving by a different door.
        assert_eq!(
            sup.started_excluding(RunId::from_bytes([0; 16])).runs,
            0,
            "no agent is going here, whatever the map still remembers"
        );

        // And the tick really does take it up, rather than merely listing it. This fixture's
        // checkpoint names a transcript no machine here has and there is no fleet to fetch it
        // from, so the start gives up — and giving up hands the registration back
        // (`unregister`), which is the previous leg's entry finally leaving the map. Before the
        // fix nothing here touched it at all.
        sup.start_held_runs(Capacity::runs(1)).await;
        assert!(
            !lock(&sup.live).contains_key(&id),
            "the tick started it, failed on a transcript this machine has not got, and gave the \
             registration back"
        );
        assert_eq!(
            sup.held_but_not_started().len(),
            1,
            "so the next tick is a real retry and not a promise"
        );
    }

    #[tokio::test]
    async fn two_commitments_on_a_one_slot_node_do_not_block_each_other() {
        // The deadlock ADR-0006's accepting-without-starting had all along, and that two
        // submissions could never show: three runs granted to a one-slot machine leaves it
        // holding two commitments once the first finishes, and if a commitment counts as
        // occupying a slot then each of the two reads the *other* as the reason it cannot
        // start. Nothing fails — the leases renew, `ps` says `assigned`, and no agent ever
        // runs again.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut ids = Vec::new();
        for _ in 0..2 {
            let mut run = Run::new(
                RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
                spec(&RepoSource::parse("/tmp/whatever")),
                sup.node_id,
                now(),
            );
            run.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
            ids.push(run.id);
            // Granted while the node was full, so both are held rather than started.
            sup.take_run(run, Capacity::runs(0))
                .await
                .expect("accepted");
        }
        assert!(lock(&sup.live).is_empty(), "neither has an agent yet");

        // One slot, now free. Exactly one starts; the other is still waiting for it, which is
        // the difference between a queue and a deadlock.
        sup.start_held_runs(Capacity::runs(1)).await;
        assert_eq!(lock(&sup.live).len(), 1);
        assert!(ids.iter().any(|id| lock(&sup.live).contains_key(id)));
    }

    #[tokio::test]
    async fn the_run_that_is_due_first_takes_the_slot_that_frees() {
        // The only ordering in the system, and the only place one exists: nodes bid
        // independently and there is no queue to serialise (ADR-0013). Two commitments and one
        // slot, and the tick has to pick — so it picks the run in the most trouble rather than
        // whichever row SQLite hands back first.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut leisurely = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        leisurely.spec.deadline = Some(now() + Millis::from_mins(240));
        leisurely
            .assign(sup.node_id, now(), LEASE_TTL)
            .expect("assign");
        sup.take_run(leisurely, Capacity::runs(0))
            .await
            .expect("held");

        // Submitted second, due much sooner. Nothing about the store's order says so.
        let mut pressing = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        pressing.spec.deadline = Some(now() + Millis::from_mins(5));
        pressing
            .assign(sup.node_id, now(), LEASE_TTL)
            .expect("assign");
        let urgent = pressing.id;
        sup.take_run(pressing, Capacity::runs(0))
            .await
            .expect("held");

        assert_eq!(
            sup.held_but_not_started().first().map(|r| r.id),
            Some(urgent),
            "the run with the least slack goes first"
        );

        // One slot, so exactly one starts, and it is that one.
        sup.start_held_runs(Capacity::runs(1)).await;
        let live = lock(&sup.live);
        assert!(live.contains_key(&urgent));
        assert_eq!(live.len(), 1);
    }

    #[tokio::test]
    async fn a_heavy_run_keeps_the_slot_it_is_in_and_a_lone_one_starts_anyway() {
        // Two halves of ADR-0013's budget, both on the start path — where the count is of
        // *running agents* rather than of held runs, because answering that question with the
        // other one is the deadlock above.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut heavy = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        heavy.spec.demand = offload_core::Demand::Heavy;
        heavy.assign(sup.node_id, now(), LEASE_TTL).expect("assign");
        let big = heavy.id;

        // A two-run laptop's budget is two ordinary runs — which one heavy run spends whole.
        // It still starts, because there is nothing beside it: a budget limits what runs *with*
        // something, and a machine that refused a run bigger than its own budget would make
        // that run unplaceable rather than merely slow.
        let laptop = Capacity::runs(2);
        assert!(offload_core::Demand::Heavy.shares() >= laptop.budget);
        sup.take_run(heavy, laptop).await.expect("accepted");
        assert!(
            lock(&sup.live).contains_key(&big),
            "a lone run starts whatever it costs"
        );

        // Now the machine is accounted for. The count says there is a slot; the budget says
        // there is not, and that is the whole point of having both.
        let mut ordinary = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        ordinary
            .assign(sup.node_id, now(), LEASE_TTL)
            .expect("assign");
        let small = ordinary.id;
        sup.take_run(ordinary, laptop).await.expect("committed");
        assert_eq!(
            sup.run(small).expect("recorded").state.name(),
            "assigned",
            "held rather than started, and rather than refused"
        );
        assert!(sup.start_held_runs(laptop).await.is_empty());

        // The heavy one finishes, and the shares come back with it.
        let mut done = sup.run(big).expect("the heavy one");
        let epoch = done.epoch;
        done.complete(sup.node_id, epoch, now()).expect("complete");
        store.save_run(&done).expect("save");
        lock(&sup.live).remove(&big);

        sup.start_held_runs(laptop).await;
        assert!(lock(&sup.live).contains_key(&small), "room now, so it goes");
    }

    /// A finished run is published until it has been out in an exchange, not only for the tail:
    /// the emulator cancelled a run before meeting anybody and the fleet never heard (session
    /// ninety-four). Past the tail and not yet told, it is still published; once an exchange has
    /// happened since, it is not; and past the horizon it never is.
    #[tokio::test]
    async fn a_finished_run_is_published_until_it_has_been_told_not_only_for_the_tail() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let finished_ago = |ago: Millis| {
            let mut run = Run::new(
                RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
                spec(&RepoSource::parse("/tmp/whatever")),
                sup.node_id,
                now(),
            );
            run.cancel(now().saturating_sub(ago)).expect("cancel");
            store.save_run(&run).expect("save");
            run.id
        };
        let hour = Millis(60 * 60 * 1_000);
        let quiet = finished_ago(hour);
        let ancient = finished_ago(Millis(30 * hour.0));
        let ids = |runs: Vec<Run>| runs.into_iter().map(|r| r.id).collect::<Vec<_>>();

        assert!(
            !ids(sup.gossipable_runs()).contains(&quiet),
            "the tail alone would have stopped publishing it"
        );
        let first = ids(sup.runs_to_publish(7));
        assert!(
            first.contains(&quiet),
            "an hour past the tail, and nobody told yet"
        );
        assert!(!first.contains(&ancient), "past the horizon it is not news");
        assert!(
            ids(sup.runs_to_publish(7)).contains(&quiet),
            "no exchange yet: still published"
        );
        assert!(
            !ids(sup.runs_to_publish(8)).contains(&quiet),
            "an exchange since: told"
        );
        assert!(
            !ids(sup.runs_to_publish(9)).contains(&quiet),
            "and it stays told"
        );

        // A peer that was away sends its stale `pending` copy: tell it again, until the next
        // exchange, which is the one that reaches the peer that is behind.
        sup.retell(quiet);
        assert!(
            ids(sup.runs_to_publish(9)).contains(&quiet),
            "published again for the laggard"
        );
        assert!(
            !ids(sup.runs_to_publish(10)).contains(&quiet),
            "and told again after one exchange"
        );
    }

    /// A stale live copy of a finished run is refused by the store, not only by the view: the
    /// view forgets finished runs after the tail, and a device that had been away reopened a
    /// cancelled run on the laptop by gossiping its `pending` copy (session ninety-four). The
    /// finished record is told again for it. A *higher* epoch is not stale, and still wins.
    #[tokio::test]
    async fn a_stale_live_copy_cannot_reopen_a_finished_run_in_the_store() {
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let pending = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            NodeId::from_bytes([9; 32]),
            now(),
        );
        let mut cancelled = pending.clone();
        cancelled
            .cancel(now().saturating_sub(Millis(60 * 60 * 1_000)))
            .expect("cancel");
        sup.record_run(&cancelled).expect("record");
        // Told already, as it would be an hour later.
        let _ = sup.runs_to_publish(1);
        assert!(!sup.runs_to_publish(2).iter().any(|r| r.id == pending.id));

        sup.record_run(&pending)
            .expect("a stale copy is not an error");
        assert_eq!(
            store
                .load_run(pending.id)
                .expect("load")
                .expect("run")
                .state
                .name(),
            "cancelled",
            "the stale copy did not reopen it"
        );
        assert!(
            sup.runs_to_publish(2).iter().any(|r| r.id == pending.id),
            "and it is told again, for the peer that is behind"
        );

        let mut reassigned = pending.clone();
        reassigned.epoch = offload_core::run::Epoch(cancelled.epoch.0 + 1);
        sup.record_run(&reassigned).expect("record");
        assert_eq!(
            store
                .load_run(pending.id)
                .expect("load")
                .expect("run")
                .state
                .name(),
            "pending",
            "a higher epoch is a later decision, and wins"
        );
    }

    #[tokio::test]
    async fn a_departing_node_hands_back_what_it_never_started() {
        // A commitment to start something later is worth exactly as much as the node that
        // made it. A draining node's is worth nothing, so it goes back to the pool rather
        // than making the drain wait out a checkpoint deadline for an agent that never
        // existed — and rather than leaving a promise behind that nothing will honour.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let (busy, _) = dormant_run(&sup, &store);

        let mut waiting = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        waiting
            .assign(sup.node_id, now(), LEASE_TTL)
            .expect("assign");
        let held = waiting.id;
        let epoch_when_held = waiting.epoch;
        sup.take_run(waiting, Capacity::runs(1))
            .await
            .expect("accepted");

        let mine: Vec<Run> = sup.gossipable_runs();
        let released = sup.release_unstarted(&mine);

        assert_eq!(released.len(), 1, "only the one nothing was started for");
        assert_eq!(released[0].id, held);
        assert_eq!(released[0].state.name(), "pending");
        assert!(
            released[0].epoch > epoch_when_held,
            "the epoch bumps, so this node stops being able to act on it"
        );
        // And the sentence that said why it had not started goes with it. It was a fact about
        // this node's slots, which stopped being about this run the moment the run stopped
        // being this node's — and it gossips, so it is what the whole fleet reads in `offload
        // ps` while the run waits for a bid round.
        assert_eq!(
            store.load_stats(held).expect("stats").workspace,
            "",
            "a released commitment still claimed to be waiting for a slot here"
        );
        assert_eq!(
            sup.run(busy).expect("the running one").state.name(),
            "running",
            "a run with an agent behind it is not handed back — that is the drain's job, at a \
             turn boundary"
        );
    }

    #[test]
    fn a_lease_is_renewed_while_this_node_is_holding_the_run() {
        // Nothing renewed a lease until this existed, so a run granted by a peer carried that
        // arbiter's 60-second lease for its whole life: the arbiter watched a perfectly
        // healthy holder, saw the lease lapse, and called the run `Orphaned` a minute after
        // granting it. Accepting-without-starting makes it load-bearing rather than untidy —
        // a held run's entire commitment is the lease.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        // Shorter than `LEASE`, standing in for the ordinary case: the arbiter's grant was
        // issued seconds ago and the renewal is measured from now, so the deadline moves.
        // (Granting a full-length lease in the same millisecond would renew it to the instant
        // it already expires, which proves nothing either way.)
        let epoch = run
            .assign(sup.node_id, now(), Millis(5_000))
            .expect("assign");
        run.started(sup.node_id, epoch, now()).expect("start");
        let before = run.state.lease().expect("lease").expires_at;
        store.save_run(&run).expect("save");

        sup.heartbeat();

        let after = sup
            .run(run.id)
            .expect("run")
            .state
            .lease()
            .expect("lease")
            .expires_at;
        assert!(after > before, "{after:?} should be later than {before:?}");
        assert_eq!(
            sup.run(run.id).expect("run").epoch,
            epoch,
            "renewing is free"
        );
    }

    #[test]
    fn a_run_placed_on_a_peer_is_not_this_nodes_load() {
        // Found on two real daemons: submit six runs that all go to the other node, and the
        // submitting node reports `runs 6/2, at capacity` while sitting completely idle —
        // because it counted every unfinished run in its store, and recording where work went
        // is the whole point of having those rows.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let elsewhere = NodeId::from_bytes([7; 32]);

        for holder in [sup.node_id, elsewhere, elsewhere] {
            let mut run = Run::new(
                RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
                spec(&RepoSource::parse("/tmp/whatever")),
                sup.node_id,
                now(),
            );
            let epoch = run.assign(holder, now(), Millis(60_000)).expect("assign");
            run.started(holder, epoch, now()).expect("start");
            store.save_run(&run).expect("save");
        }

        assert_eq!(
            sup.held_count(),
            1,
            "one run is here; two are somebody else's"
        );
    }

    #[test]
    fn a_run_held_somewhere_else_is_not_ours_to_renew() {
        // The heartbeat is a claim about what this node is doing. Renewing a peer's lease
        // would be this node vouching for a machine it cannot see.
        let store = Store::open_memory().expect("store");
        let sup = supervisor_with(store.clone());
        let elsewhere = NodeId::from_bytes([7; 32]);

        let mut run = Run::new(
            RunId::from_bytes(*uuid::Uuid::now_v7().as_bytes()),
            spec(&RepoSource::parse("/tmp/whatever")),
            sup.node_id,
            now(),
        );
        let epoch = run
            .assign(elsewhere, now(), Millis(60_000))
            .expect("assign");
        run.started(elsewhere, epoch, now()).expect("start");
        let before = run.state.lease().expect("lease").expires_at;
        store.save_run(&run).expect("save");

        sup.heartbeat();

        assert_eq!(
            sup.run(run.id)
                .expect("run")
                .state
                .lease()
                .expect("lease")
                .expires_at,
            before
        );
    }

    #[test]
    fn prompts_are_flattened_and_truncated_for_display() {
        // ps output is a table; an embedded newline would break the row.
        assert_eq!(truncate("one\ntwo", 60), "one two");
        let long = "x".repeat(100);
        let short = truncate(&long, 10);
        assert_eq!(short.chars().count(), 10);
        assert!(short.ends_with('…'));
    }
}
