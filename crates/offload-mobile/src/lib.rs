//! The Android app's window onto `offloadd` (ADR-0071).
//!
//! Kotlin calls this through UniFFI; this calls the daemon through `offload_node::client`, the same
//! client the CLI uses. What it hands back are **screen models** built here from the daemon's own
//! response types, so the compiler checks every field the app shows against the protocol, and a
//! sentence the daemon wrote — a refusal, a note — reaches the screen as the daemon wrote it. The
//! app decides nothing about the fleet; it shows what the node decided.

use offload_node::api::{LightAnswer, Request, Response};
use std::path::PathBuf;
use std::sync::Arc;

uniffi::setup_scaffolding!();

/// Why a call failed, as a sentence for the screen.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    /// The daemon could not be reached, or answered with an error; the reason says which.
    ///
    /// `reason`, not `message`: UniFFI turns this into a Kotlin exception, and a field called
    /// `message` collides with `Throwable.message` — the generated file did not compile.
    #[error("{reason}")]
    Daemon { reason: String },
}

impl MobileError {
    fn from_anyhow(e: &anyhow::Error) -> Self {
        // `{:#}` keeps the context chain — "connecting to …: permission denied" — on one line.
        MobileError::Daemon {
            reason: format!("{e:#}"),
        }
    }
}

/// A connection to one daemon's control socket.
#[derive(Debug, uniffi::Object)]
pub struct Daemon {
    socket: PathBuf,
    runtime: tokio::runtime::Runtime,
}

#[uniffi::export]
impl Daemon {
    /// `socket` is the daemon's control socket; the app's is `<filesDir>/s/offloadd.sock`.
    #[uniffi::constructor]
    pub fn new(socket: String) -> Result<Arc<Self>, MobileError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| MobileError::Daemon {
                reason: format!("cannot start a runtime: {e}"),
            })?;
        Ok(Arc::new(Daemon {
            socket: PathBuf::from(socket),
            runtime,
        }))
    }

    /// What this node is and whether it would take work — `offload status`.
    pub fn status(&self) -> Result<StatusView, MobileError> {
        match self.one(&Request::Status)? {
            Response::Status(s) => Ok(StatusView::from(*s)),
            other => Err(unexpected(&other)),
        }
    }

    /// Who else is in the fleet, and how each is doing — `offload nodes`.
    pub fn nodes(&self) -> Result<NodesView, MobileError> {
        match self.one(&Request::Nodes)? {
            Response::Nodes { nodes, local } => Ok(NodesView {
                in_mesh: local.is_some(),
                nodes: nodes
                    .into_iter()
                    .map(|n| NodeRow {
                        this_node: local.as_deref() == Some(n.id.to_string().as_str()),
                        id: n.id.to_string(),
                        short_id: n.id.short(),
                        name: n.name,
                        status: n.status.into(),
                        device_class: n.device_class,
                        cpu_cores: n.cpu_cores,
                        memory_mb: n.memory_mb,
                        agent: n.agent,
                        running: n.running,
                        last_heard_ms: n.last_heard_ms,
                        absences: n.absences,
                        programs: n.programs,
                        may_host: n.may_host,
                        models: n
                            .models
                            .into_iter()
                            .map(|m| ModelRow {
                                value: m.value,
                                name: m.name,
                                about: m.about,
                            })
                            .collect(),
                    })
                    .collect(),
            }),
            other => Err(unexpected(&other)),
        }
    }

    /// Read this device's model list again and ask the fleet to do the same (ADR-0080) —
    /// `offload models --refresh`. Whether the request travels, which it cannot without a mesh.
    pub fn refresh_models(&self) -> Result<bool, MobileError> {
        match self.one(&Request::RefreshModels)? {
            Response::ModelsAsked { fleet } => Ok(fleet),
            other => Err(unexpected(&other)),
        }
    }
}

#[uniffi::export]
impl Daemon {
    /// Every run this node knows of, newest first as the daemon lists them — `offload ps --all`.
    pub fn runs(&self) -> Result<RunsView, MobileError> {
        match self.one(&Request::List)? {
            Response::Runs {
                runs,
                resume_refusal,
            } => Ok(RunsView {
                runs: runs.into_iter().map(RunRow::from).collect(),
                resume_refusal,
            }),
            other => Err(unexpected(&other)),
        }
    }

    /// A run's log so far, as the lines `offload logs` prints — worded by `offload_node::render`,
    /// the one place both clients take it from.
    pub fn log(&self, run: String) -> Result<Vec<String>, MobileError> {
        let responses = self
            .runtime
            .block_on(offload_node::client::request(
                &self.socket,
                &Request::Logs {
                    run: run.clone(),
                    follow: false,
                },
            ))
            .map_err(|e| MobileError::from_anyhow(&e))?;
        let now = now_ms();
        let mut lines = Vec::new();
        for response in responses {
            match response {
                Response::Event(event) => {
                    for line in offload_node::render::event_lines(&event.kind, &run, now) {
                        // A line the CLI prints with an embedded break is two lines on a screen.
                        lines.extend(line.split('\n').map(str::to_string));
                    }
                }
                Response::Error { message } => return Err(MobileError::Daemon { reason: message }),
                _ => {}
            }
        }
        Ok(lines)
    }

    /// Submit a task: a service the fleet offers, run by whichever node takes it (ADR-0019).
    ///
    /// `queue` leaves it pending when nobody takes it now, rather than refusing — the CLI's
    /// `--queue`, which the fleet's refusal names.
    pub fn submit_task(
        &self,
        service: String,
        args: Vec<String>,
        queue: bool,
    ) -> Result<Submitted, MobileError> {
        let service = service
            .trim()
            .parse::<offload_core::Service>()
            .map_err(|e| MobileError::Daemon {
                reason: e.to_string(),
            })?;
        self.submitted(&Request::SubmitTask(offload_node::api::SubmitTaskRequest {
            service,
            args,
            queue,
            deadline: None,
            demand: offload_core::Demand::default(),
            notify: offload_core::Audience::default(),
            notices: offload_core::Notices::default(),
            require: offload_core::Wanted::default(),
            prefer: offload_core::Wanted::default(),
            hold_until: None,
            // Typed by a person, which decides whether its output may be thrown away (ADR-0024).
            origin: offload_core::Origin::Operator,
        }))
    }

    /// Submit an agent run on a repository any node can fetch — a URL, since a path on a phone
    /// means nothing to the node that takes it (`offload_core::repo`) — or, with `repo` empty, in
    /// an empty workspace (ADR-0072).
    ///
    /// `ask` is the CLI's `--ask`: the run stops and asks a person before a call it is not already
    /// permitted, up to the default budget of questions, under the `ask` permission mode
    /// (ADR-0017). Its questions then arrive on the Questions screen.
    pub fn submit_agent(
        &self,
        repo: String,
        prompt: String,
        model: Option<String>,
        queue: bool,
        ask: bool,
    ) -> Result<Submitted, MobileError> {
        self.submitted(&Request::Submit(offload_node::api::SubmitRequest {
            // No repository is the ordinary case from a phone: an empty workspace, made by the
            // node that takes the run (ADR-0072).
            repo: match repo.trim() {
                "" => offload_core::SCRATCH.to_string(),
                given => given.to_string(),
            },
            prompt,
            model: model.filter(|m| !m.trim().is_empty()),
            permission: ask.then_some(offload_core::PermissionMode::Ask),
            git_ref: None,
            allow: Vec::new(),
            queue,
            deadline: None,
            demand: offload_core::Demand::default(),
            notify: offload_core::Audience::default(),
            notices: offload_core::Notices::default(),
            ask: if ask {
                offload_core::AskPolicy::UpTo {
                    questions: offload_core::DEFAULT_ASK_BUDGET,
                }
            } else {
                offload_core::AskPolicy::Never
            },
            max_turns: None,
            resources: Vec::new(),
            require: offload_core::Wanted::default(),
            prefer: offload_core::Wanted::default(),
            hold_until: None,
            origin: offload_core::Origin::Operator,
        }))
    }

    /// What is waiting for a person right now, on any node — `offload asks`. Asked live each
    /// time, never remembered: a question somebody answered elsewhere must not stay on screen.
    pub fn asks(&self) -> Result<Vec<AskRow>, MobileError> {
        match self.one(&Request::Asks)? {
            Response::Asks { asks } => Ok(asks
                .into_iter()
                .map(|a| AskRow {
                    run: a.run.to_string(),
                    short_run: short(&a.run.to_string()),
                    node: a.node_name.or_else(|| a.node.map(|n| n.short())),
                    tool_use_id: a.tool_use_id,
                    tool: a.tool,
                    detail: a.detail,
                    waiting: a.waiting.to_string(),
                    left: a.left.to_string(),
                    left_ms: a.left.0,
                })
                .collect()),
            other => Err(unexpected(&other)),
        }
    }

    /// Answer one question — `offload approve|deny <run> <tool_use_id>`. Addressed to the agent's
    /// own call id, so the answer lands on the call it was about and no other.
    pub fn answer(
        &self,
        run: String,
        tool_use_id: String,
        allow: bool,
    ) -> Result<String, MobileError> {
        match self.one(&Request::Answer {
            run,
            tool_use_id: Some(tool_use_id),
            allow,
        })? {
            Response::Answered {
                tool,
                detail,
                allowed,
            } => Ok(format!(
                "{} {tool}: {detail}",
                if allowed { "allowed" } else { "denied" }
            )),
            other => Err(unexpected(&other)),
        }
    }

    /// A new run on a finished one's work — `offload continue` (ADR-0064). It starts on the
    /// parent's branch, so it sees what the parent left: its files, its commits. `session` resumes
    /// the parent's conversation; otherwise a fresh one is handed the parent's prompt and result.
    /// Everything else is the parent's, which is the ADR's inheritance table. The refusals (a
    /// task, a run still going) are the daemon's words.
    pub fn continue_run(
        &self,
        run: String,
        prompt: String,
        session: bool,
        queue: bool,
    ) -> Result<Submitted, MobileError> {
        self.submitted(&Request::Continue(Box::new(
            offload_node::api::ContinueRequest {
                run,
                prompt,
                mode: if session {
                    offload_core::ContinueMode::Session
                } else {
                    offload_core::ContinueMode::Handoff
                },
                queue,
                deadline: None,
                allow: Vec::new(),
                max_turns: None,
                ask: offload_core::AskPolicy::default(),
                require: None,
                prefer: None,
                hold_until: None,
            },
        )))
    }

    /// Why a run is where it is — the part of `offload explain` a waiting run needs: the
    /// arbiter's verdict and what each node said when asked to take it, in the daemon's words.
    /// A pending run showed "pending" and nothing else, while the fleet knew exactly why nobody
    /// took it (the owner's email run on the tablet, session ninety-two).
    pub fn why(&self, run: String) -> Result<WhyView, MobileError> {
        match self.one(&Request::Explain { run })? {
            Response::Explanation(e) => Ok(WhyView {
                verdict: e.verdict,
                not_asked: e.not_canvassed,
                answers: e
                    .opinions
                    .into_iter()
                    .map(|o| NodeAnswer {
                        node: o.node,
                        says: o.verdict,
                        would_take: o.bidding,
                    })
                    .collect(),
            }),
            other => Err(unexpected(&other)),
        }
    }

    /// What is at `path` in a run's workspace, read-only (ADR-0075) — `offload files`. `""` is
    /// the root. Read on the machine that has it, which the view names.
    pub fn files(&self, run: String, path: String) -> Result<FilesScreen, MobileError> {
        use offload_node::api::{FilesContent, FilesSource};
        match self.one(&Request::Files { run, path })? {
            Response::Files { view } => {
                let mut screen = FilesScreen {
                    path: view.path,
                    node: view.node,
                    from_checkout: view.source == FilesSource::Checkout,
                    listing: false,
                    entries: Vec::new(),
                    text: None,
                    bytes: 0,
                    truncated: false,
                };
                match view.content {
                    FilesContent::Listing { entries, truncated } => {
                        screen.listing = true;
                        screen.truncated = truncated;
                        screen.entries = entries
                            .into_iter()
                            .map(|e| FileRow {
                                name: e.name,
                                dir: e.dir,
                                bytes: e.bytes,
                            })
                            .collect();
                    }
                    FilesContent::Text {
                        text,
                        bytes,
                        truncated,
                    } => {
                        screen.text = Some(text);
                        screen.bytes = bytes;
                        screen.truncated = truncated;
                    }
                    FilesContent::Binary { bytes } => screen.bytes = bytes,
                }
                Ok(screen)
            }
            other => Err(unexpected(&other)),
        }
    }

    /// Stop a run, wherever it is — `offload cancel`. The answer is the daemon's note.
    pub fn cancel(&self, run: String) -> Result<String, MobileError> {
        match self.one(&Request::Cancel { run })? {
            Response::Cancelled { run, node, note } => Ok(match node {
                Some(node) => format!("cancelled {} on {node}: {note}", short(&run)),
                None => format!("cancelled {}: {note}", short(&run)),
            }),
            other => Err(unexpected(&other)),
        }
    }
}

impl Daemon {
    fn submitted(&self, request: &Request) -> Result<Submitted, MobileError> {
        match self.one(request)? {
            Response::Submitted {
                run,
                node,
                waiting,
                queued,
                ..
            } => Ok(Submitted {
                short_id: short(&run),
                run,
                node,
                waiting,
                queued,
            }),
            other => Err(unexpected(&other)),
        }
    }

    /// One request, one answer, with a daemon-side error turned into ours.
    fn one(&self, request: &Request) -> Result<Response, MobileError> {
        let responses = self
            .runtime
            .block_on(offload_node::client::request(&self.socket, request))
            .map_err(|e| MobileError::from_anyhow(&e))?;
        offload_node::client::expect_one(responses).map_err(|e| MobileError::from_anyhow(&e))
    }
}

/// A run id's short form, as `offload ps` prints it.
fn short(run: &str) -> String {
    run.chars().take(12).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `offload ps`, for a screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RunsView {
    pub runs: Vec<RunRow>,
    /// Why `resume` would be refused on this node, when a listed run is one somebody would
    /// resume — the node's own sentence, beside the runs rather than baked into them.
    pub resume_refusal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RunRow {
    pub id: String,
    pub short_id: String,
    /// As the daemon words it: `running`, `completed`, `failed`, `pending`…
    pub state: String,
    /// Over, by the rule `offload ps` hides finished runs by.
    pub finished: bool,
    pub task: bool,
    /// The prompt's first line, or the task's service.
    pub work: String,
    pub repo: String,
    pub turns: u32,
    pub tokens: u64,
    pub cost_micro_usd: u64,
    pub started_at_unix: u64,
    pub error: Option<String>,
    pub due: Option<String>,
    pub held: Option<String>,
    pub resumable: bool,
    /// The run this one continues (ADR-0064), in short form, if it continues one.
    pub continues: Option<String>,
    /// A rule or a schedule started it, not a person (ADR-0024). The home list leaves out such
    /// a run's successful tasks, and never one somebody started.
    pub by_rule: bool,
    /// The node that has it, or last worked on it; `None` if nobody has taken it yet.
    pub host: Option<String>,
    /// The model it asked for; `None` for the agent's default.
    pub model: Option<String>,
}

impl From<offload_node::api::RunSummary> for RunRow {
    fn from(r: offload_node::api::RunSummary) -> Self {
        RunRow {
            short_id: short(&r.id),
            id: r.id,
            finished: offload_node::render::is_finished(&r.state),
            state: r.state,
            task: r.kind == offload_core::WorkKind::Task,
            work: r.work,
            repo: r.repo,
            turns: r.turns,
            tokens: r.tokens.total(),
            cost_micro_usd: r.cost_micro_usd,
            started_at_unix: r.started_at_unix,
            error: r.error,
            due: r.due,
            held: r.held,
            resumable: r.resumable,
            continues: r.continues.as_deref().map(short),
            by_rule: r.origin == offload_core::Origin::Rule,
            host: r.host,
            model: r.model,
        }
    }
}

/// A directory or a file in a run's workspace (ADR-0075), for a screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FilesScreen {
    /// Relative to the workspace root, `""` for the root.
    pub path: String,
    /// The machine it was read on.
    pub node: String,
    /// The checkout, with uncommitted work; otherwise the run's branch.
    pub from_checkout: bool,
    /// A directory, whose entries follow; otherwise a file.
    pub listing: bool,
    pub entries: Vec<FileRow>,
    /// A text file's contents; `None` for a directory or a file that is not text.
    pub text: Option<String>,
    /// A file's whole size.
    pub bytes: u64,
    /// The listing or the text was cut.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FileRow {
    pub name: String,
    pub dir: bool,
    pub bytes: u64,
}

/// Why a run is where it is, from `offload explain`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct WhyView {
    /// The arbiter's sentence about the run.
    pub verdict: String,
    /// Why nobody was asked, when nobody was.
    pub not_asked: Option<String>,
    /// Each node's answer to "would you take it", best first.
    pub answers: Vec<NodeAnswer>,
}

/// One node's answer, as it worded it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NodeAnswer {
    pub node: String,
    pub says: String,
    pub would_take: bool,
}

/// One question an agent is stopped on (ADR-0017), for a screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AskRow {
    pub run: String,
    pub short_run: String,
    /// Where the run is, by name where the node knows it.
    pub node: Option<String>,
    /// The agent's own id for the call — what an answer is addressed to.
    pub tool_use_id: String,
    pub tool: String,
    /// One line describing the call, for whoever decides.
    pub detail: String,
    /// How long it has waited, and how long until nobody answering decides it, as the CLI says them.
    pub waiting: String,
    pub left: String,
    pub left_ms: u64,
}

/// What a submission came back with. A refusal — nobody will take it — is an error, worded by the
/// daemon, not one of these.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Submitted {
    pub run: String,
    pub short_id: String,
    /// Who accepted it, when somebody did.
    pub node: Option<String>,
    /// Why it has not started yet, when it has not.
    pub waiting: Option<String>,
    /// Why it was queued rather than placed, when it was.
    pub queued: Option<String>,
}

fn unexpected(response: &Response) -> MobileError {
    MobileError::Daemon {
        reason: format!("the daemon answered with something else: {response:?}"),
    }
}

/// `offload status`, for a screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StatusView {
    pub node_id: String,
    pub name: String,
    pub device_class: String,
    /// The agent's version, if one was found; `agent_binary` is what was looked for.
    pub agent: Option<String>,
    pub agent_authenticated: bool,
    pub agent_binary: String,
    pub running: u32,
    /// What each run held here is doing, one line each (ADR-0079).
    pub holding: Vec<String>,
    pub max_concurrent: u32,
    /// `None` where the platform will not say, which is not the same as idle.
    pub cpu_percent: Option<u8>,
    pub thermal: Option<String>,
    pub approval_key: Option<String>,
    /// Why this node would refuse an ordinary run now, in the daemon's words; `None` accepts.
    pub refusal: Option<String>,
    /// The owner's separate answer for light work, when there is one.
    pub light: Option<LightView>,
    /// Runs stopped waiting for a person to answer a question.
    pub asks: u32,
    pub fleet: Option<FleetView>,
}

/// The owner's answer for light work (ADR-0019 §4), when it differs.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum LightView {
    Accepted,
    Refused { reason: String },
}

/// This node's fleet, as `health` judged it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FleetView {
    pub fleet: String,
    pub members_met: u32,
    pub approvers_met: u32,
    pub cert_days: u64,
    pub revoked_here: bool,
    /// Peers whose certificates are running out, as "name in N days".
    pub expiring: Vec<String>,
    /// Memberships whose approval runs out within a month (ADR-0069 §3).
    pub reapproval_due: Vec<ApprovalDueRow>,
    /// The nags, each a sentence with its own thing to do.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ApprovalDueRow {
    pub node: String,
    pub name: String,
    pub days: u64,
    pub decided: bool,
}

impl From<offload_node::api::NodeStatus> for StatusView {
    fn from(s: offload_node::api::NodeStatus) -> Self {
        StatusView {
            node_id: s.node_id,
            name: s.name,
            device_class: s.device_class,
            agent: s.agent,
            agent_authenticated: s.agent_authenticated,
            agent_binary: s.agent_binary,
            running: s.running,
            holding: s.holding,
            max_concurrent: s.max_concurrent,
            cpu_percent: s.cpu_percent,
            thermal: s.thermal,
            approval_key: s.approval_key,
            refusal: s.refusal,
            light: s.light_refusal.map(|answer| match answer {
                LightAnswer::Accepted => LightView::Accepted,
                LightAnswer::Refused { reason } => LightView::Refused { reason },
            }),
            asks: s.asks,
            fleet: s.fleet.map(|f| FleetView {
                fleet: f.fleet,
                members_met: f.members_met,
                approvers_met: f.approvers_met,
                cert_days: f.cert_days,
                revoked_here: f.revoked_here,
                expiring: f.expiring,
                reapproval_due: f
                    .reapproval_due
                    .into_iter()
                    .map(|r| ApprovalDueRow {
                        node: r.node.to_string(),
                        name: r.name,
                        days: r.days,
                        decided: r.decided,
                    })
                    .collect(),
                notes: f.notes,
            }),
        }
    }
}

/// `offload nodes`, for a screen.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NodesView {
    /// `false` when this node has no mesh at all — a different answer from a mesh of one.
    pub in_mesh: bool,
    pub nodes: Vec<NodeRow>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NodeRow {
    pub this_node: bool,
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub status: PeerStatus,
    pub device_class: String,
    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub agent: Option<String>,
    pub running: u32,
    /// Since this device last heard from it first-hand; `None` if it never has.
    pub last_heard_ms: Option<u64>,
    /// Times it has dropped off and come back; a node that returns quickly earns patience.
    pub absences: u32,
    /// The programs it offers to run (tasks, ADR-0019), for the composer to offer.
    pub programs: Vec<String>,
    /// Whether the fleet lets it host runs, from its certificate; `None` when unknown here.
    pub may_host: Option<bool>,
    /// The models its agent offers, in the agent's order (ADR-0080), for the model picker.
    pub models: Vec<ModelRow>,
}

/// A model an agent offers, in the agent's own words (ADR-0080).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ModelRow {
    /// What `--model` takes. `default` means the agent's own default, which is no `--model`.
    pub value: String,
    pub name: String,
    pub about: String,
}

/// A peer's liveness, as the failure detector holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PeerStatus {
    Alive,
    Suspect,
    Dead,
    Draining,
    Departed,
}

impl From<offload_core::NodeStatus> for PeerStatus {
    fn from(s: offload_core::NodeStatus) -> Self {
        match s {
            offload_core::NodeStatus::Alive => PeerStatus::Alive,
            offload_core::NodeStatus::Suspect => PeerStatus::Suspect,
            offload_core::NodeStatus::Dead => PeerStatus::Dead,
            offload_core::NodeStatus::Draining => PeerStatus::Draining,
            offload_core::NodeStatus::Departed => PeerStatus::Departed,
        }
    }
}

/// This device's own files: its identity, membership, approval key and work policy (ADR-0071
/// slice 4). Read and written in the state directory and the config, the way `offload id`, `join`
/// and `fleet` work with no daemon; the app restarts its daemon after a change that needs it.
#[derive(Debug, uniffi::Object)]
pub struct Device {
    state_dir: PathBuf,
    config: PathBuf,
}

#[uniffi::export]
impl Device {
    #[uniffi::constructor]
    pub fn new(state_dir: String, config: String) -> Arc<Self> {
        Arc::new(Device {
            state_dir: PathBuf::from(state_dir),
            config: PathBuf::from(config),
        })
    }

    /// This device's node id — what an approver needs to invite it. Made if there is none yet,
    /// as `offload id` does, so the id shown is the one an invitation will name.
    pub fn node_id(&self) -> Result<String, MobileError> {
        offload_node::identity::load_or_create(&self.state_dir)
            .map(|identity| identity.id().to_string())
            .map_err(|e| MobileError::Daemon {
                reason: format!("node identity: {e}"),
            })
    }

    /// This device's membership, or `None` when it belongs to no fleet.
    pub fn membership(&self) -> Result<Option<MembershipView>, MobileError> {
        let now = now_millis();
        let state =
            offload_node::fleet::load(&self.state_dir).map_err(|e| MobileError::Daemon {
                reason: e.to_string(),
            })?;
        let key = offload_node::approval_key::read(&self.state_dir);
        Ok(state.map(|state| {
            let named_key = key.as_ref().and_then(|k| k.issuer_key().ok());
            MembershipView {
                fleet: state.fleet.to_string(),
                name: state.membership.name.clone(),
                grants: state.grants(now).iter().map(ToString::to_string).collect(),
                approver: state.membership.granted(offload_core::Grant::Approve, now)
                    && state.approver.is_some(),
                approves_with_hardware_key: state
                    .approver
                    .as_ref()
                    .is_some_and(|d| !d.issuer_key.is_node()),
                this_key_named: named_key.is_some_and(|key| {
                    state.approver.as_ref().is_some_and(|d| d.issuer_key == key)
                }),
                cert_expires_unix_ms: state.membership.expires_at.0,
            }
        }))
    }

    /// The approval key the app holds, as the app verified it (`approval-key.json`).
    pub fn approval_key(&self) -> Option<String> {
        offload_node::approval_key::read(&self.state_dir).map(|k| k.level().to_string())
    }

    /// What an invitation says, before anything is done with it — for the confirmation a tapped
    /// link has to pass through. A link can come from anybody, so joining is never the tap itself.
    /// Accepts the token or the `offload://join?token=` link around it.
    pub fn inspect_invite(&self, token: String) -> Result<InviteView, MobileError> {
        let token = token_of(&token);
        let invitation =
            offload_node::fleet::decode_invite(&token).map_err(|e| MobileError::Daemon {
                reason: e.to_string(),
            })?;
        let me = self.node_id()?;
        let membership = &invitation.credentials.membership;
        let current = offload_node::fleet::load(&self.state_dir)
            .ok()
            .flatten()
            .map(|s| s.fleet.to_string());
        Ok(InviteView {
            token,
            fleet: membership.fleet.to_string(),
            name: membership.name.clone(),
            grants: membership.grants.iter().map(ToString::to_string).collect(),
            for_this_device: membership.member.to_string() == me,
            current_fleet: current,
        })
    }

    /// Take up an invitation (`offload join --token`), by the rules the CLI uses. The answer is
    /// what happened, in a sentence; a refusal is an error, worded the same way.
    pub fn join(&self, token: String) -> Result<String, MobileError> {
        use offload_node::fleet::TakenUp;
        let taken = offload_node::fleet::take_up_invitation(
            &self.state_dir,
            &token_of(&token),
            now_millis(),
        )
        .map_err(|reason| MobileError::Daemon { reason })?;
        Ok(match taken {
            TakenUp::Joined(state) => format!(
                "Joined fleet {} as {}.",
                state.fleet.short(),
                state.membership.name
            ),
            TakenUp::Moved { from, state } => format!(
                "Moved from fleet {} to {}.",
                from.short(),
                state.fleet.short()
            ),
            TakenUp::NewCertificate(state) => {
                format!(
                    "Took up a new certificate in fleet {}.",
                    state.fleet.short()
                )
            }
        })
    }

    /// Make sure the config has the app's own delivery route (ADR-0010): a sink that files each
    /// notification in `dir` for the app to show, so a question reaches this phone with the app
    /// closed. `true` when it was added — the daemon reads sinks at start, so the caller restarts
    /// it. An owner's existing sinks are left as they are; one with the id `app` is theirs.
    pub fn ensure_app_sink(&self, dir: String) -> Result<bool, MobileError> {
        let bad = |reason: String| MobileError::Daemon { reason };
        let text = std::fs::read_to_string(&self.config)
            .map_err(|e| bad(format!("reading {}: {e}", self.config.display())))?;
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| bad(format!("{} is not valid TOML: {e}", self.config.display())))?;
        let has_app = doc
            .get("sinks")
            .and_then(toml_edit::Item::as_array_of_tables)
            .is_some_and(|sinks| {
                sinks
                    .iter()
                    .any(|t| t.get("id").and_then(toml_edit::Item::as_str) == Some("app"))
            });
        if has_app {
            return Ok(false);
        }
        let mut sink = toml_edit::Table::new();
        sink["id"] = toml_edit::value("app");
        sink["service"] = toml_edit::value("push");
        sink["command"] = toml_edit::value("/system/bin/sh");
        let mut args = toml_edit::Array::new();
        for arg in app_sink_args(&dir) {
            args.push(arg);
        }
        sink["args"] = toml_edit::value(args);
        sink["description"] = toml_edit::value("this phone's notifications, shown by the app");
        doc.entry("sinks")
            .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or_else(|| bad("`sinks` in the config is not a list of tables".into()))?
            .push(sink);
        let tmp = self.config.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string())
            .map_err(|e| bad(format!("writing the config: {e}")))?;
        if let Err(e) = offload_node::config::Config::load(Some(&tmp)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(bad(format!("the daemon would refuse that config: {e}")));
        }
        std::fs::rename(&tmp, &self.config).map_err(|e| bad(format!("saving the config: {e}")))?;
        Ok(true)
    }

    /// The owner's work policy, as the config says it — the answers the daemon reads.
    pub fn policy(&self) -> Result<PolicyView, MobileError> {
        let config = offload_node::config::Config::load(Some(&self.config)).map_err(|e| {
            MobileError::Daemon {
                reason: e.to_string(),
            }
        })?;
        let policy = config.policy.unwrap_or_default();
        Ok(PolicyView {
            accept: policy.accept.map(|a| accept_word(a).to_string()),
            min_battery_percent: policy.min_battery_percent,
            allow_metered: policy.allow_metered,
        })
    }

    /// Change the owner's work policy in `node.toml`, leaving everything else as written. `None`
    /// removes a setting, so the device's own default applies again. The daemon reads its config
    /// at start, so the app restarts it afterwards.
    pub fn set_policy(
        &self,
        accept: Option<String>,
        min_battery_percent: Option<u8>,
        allow_metered: Option<bool>,
    ) -> Result<(), MobileError> {
        let bad = |reason: String| MobileError::Daemon { reason };
        let text = std::fs::read_to_string(&self.config)
            .map_err(|e| bad(format!("reading {}: {e}", self.config.display())))?;
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| bad(format!("{} is not valid TOML: {e}", self.config.display())))?;
        let policy = doc
            .entry("policy")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| bad("[policy] in the config is not a table".into()))?;
        match accept {
            Some(a) => {
                if !matches!(a.as_str(), "never" | "when_charging" | "always") {
                    return Err(bad(format!(
                        "accept is never, when_charging or always, not `{a}`"
                    )));
                }
                policy["accept"] = toml_edit::value(a);
            }
            None => {
                policy.remove("accept");
            }
        }
        match min_battery_percent {
            Some(p) => policy["min_battery_percent"] = toml_edit::value(i64::from(p)),
            None => {
                policy.remove("min_battery_percent");
            }
        }
        match allow_metered {
            Some(m) => policy["allow_metered"] = toml_edit::value(m),
            None => {
                policy.remove("allow_metered");
            }
        }
        // A table with nothing left in it is clutter in the owner's file, not a setting.
        if doc
            .get("policy")
            .and_then(toml_edit::Item::as_table)
            .is_some_and(toml_edit::Table::is_empty)
        {
            doc.remove("policy");
        }
        let written = doc.to_string();
        // Checked by the daemon's own loader before it is kept: a config the daemon would refuse
        // must not replace one it runs on.
        let tmp = self.config.with_extension("toml.tmp");
        std::fs::write(&tmp, &written).map_err(|e| bad(format!("writing the config: {e}")))?;
        if let Err(e) = offload_node::config::Config::load(Some(&tmp)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(bad(format!("the daemon would refuse that config: {e}")));
        }
        std::fs::rename(&tmp, &self.config).map_err(|e| bad(format!("saving the config: {e}")))
    }
}

/// The token inside an `offload://join?token=` link, or the text itself when it is a bare token.
fn token_of(text: &str) -> String {
    let text = text.trim();
    text.strip_prefix(offload_node::fleet::JOIN_LINK_PREFIX)
        .unwrap_or(text)
        .to_string()
}

/// An invitation, read but not acted on.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct InviteView {
    /// The bare token, whatever form it arrived in.
    pub token: String,
    pub fleet: String,
    /// What the fleet will call this device.
    pub name: String,
    pub grants: Vec<String>,
    /// It names this device's key. An invitation for another device cannot be taken up here.
    pub for_this_device: bool,
    /// The fleet this device already belongs to, if any.
    pub current_fleet: Option<String>,
}

/// The shell the app's sink runs under, and the script that files one notification: the JSON the
/// daemon writes on stdin, renamed into place whole so the app never reads half of one. The
/// daemon appends the summary as a last argument, which the script ignores.
fn app_sink_args(dir: &str) -> Vec<String> {
    vec![
        "-c".into(),
        format!(
            "d='{dir}'; mkdir -p \"$d\" && cat > \"$d/.$$.tmp\" && mv \"$d/.$$.tmp\" \"$d/$(date +%s)-$$.json\""
        ),
        "offload-app-sink".into(),
    ]
}

fn now_millis() -> offload_core::Millis {
    offload_core::Millis(now_ms())
}

fn accept_word(a: offload_core::AcceptWork) -> &'static str {
    match a {
        offload_core::AcceptWork::Never => "never",
        offload_core::AcceptWork::WhenCharging => "when_charging",
        offload_core::AcceptWork::Always => "always",
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MembershipView {
    pub fleet: String,
    pub name: String,
    /// In force now: a grant dormant for probation is not listed.
    pub grants: Vec<String>,
    /// Holds `approve` and the delegation that makes it usable.
    pub approver: bool,
    /// Its delegation names a key in secure hardware rather than the node key.
    pub approves_with_hardware_key: bool,
    /// …and it is the key this app holds.
    pub this_key_named: bool,
    pub cert_expires_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PolicyView {
    /// `never`, `when_charging` or `always`; `None` is the device class's default.
    pub accept: Option<String>,
    pub min_battery_percent: Option<u8>,
    pub allow_metered: Option<bool>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_daemon_is_the_clients_own_sentence() {
        let daemon = Daemon::new("/nonexistent/offloadd.sock".into()).expect("runtime");
        let Err(MobileError::Daemon { reason: message }) = daemon.status() else {
            panic!("no daemon should be an error");
        };
        assert!(message.contains("no daemon at"), "{message}");
    }

    /// The script the app's sink runs, run for real with `sh`: the notification arrives whole, as
    /// a `.json` file, with no temporary left behind.
    #[test]
    fn the_app_sink_files_one_notification_whole() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("offload-mobile-sink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut child = std::process::Command::new("sh")
            .args(app_sink_args(
                &dir.join("notifications").display().to_string(),
            ))
            .arg("the summary the daemon appends")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("sh");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(br#"{"kind":"asked","title":"run 01a0 needs a decision"}"#)
            .expect("write");
        assert!(child.wait().expect("wait").success());
        let files: Vec<_> = std::fs::read_dir(dir.join("notifications"))
            .expect("dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(
            files[0].ends_with(".json") && !files[0].starts_with('.'),
            "{files:?}"
        );
        let body =
            std::fs::read_to_string(dir.join("notifications").join(&files[0])).expect("read");
        assert!(body.contains("needs a decision"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_policy_change_keeps_the_rest_of_the_config_and_is_refused_if_the_daemon_would_refuse_it() {
        let dir =
            std::env::temp_dir().join(format!("offload-mobile-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let config = dir.join("node.toml");
        let original = "# the owner's note\nname = \"phone\"\nstate_dir = \"/tmp/x\"\n\n[cluster]\nlisten = \"[::]:7433\"\n";
        std::fs::write(&config, original).expect("write");
        let device = Device::new(
            dir.join("s").display().to_string(),
            config.display().to_string(),
        );

        device
            .set_policy(Some("always".into()), Some(30), Some(true))
            .expect("set");
        let written = std::fs::read_to_string(&config).expect("read");
        assert!(written.contains("# the owner's note"), "{written}");
        assert!(written.contains("listen = \"[::]:7433\""), "{written}");
        let policy = device.policy().expect("policy");
        assert_eq!(policy.accept.as_deref(), Some("always"));
        assert_eq!(policy.min_battery_percent, Some(30));
        assert_eq!(policy.allow_metered, Some(true));

        assert!(device
            .set_policy(Some("sometimes".into()), None, None)
            .is_err());
        device.set_policy(None, None, None).expect("clear");
        assert_eq!(device.policy().expect("policy").accept, None);
        assert!(
            !std::fs::read_to_string(&config)
                .expect("read")
                .contains("[policy]"),
            "an emptied [policy] table is removed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
