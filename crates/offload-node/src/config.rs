//! Node configuration.
//!
//! Every field has a default that produces a working node, so `offloadd` with no config
//! file at all does something sensible. Config exists to override, not to be mandatory —
//! a daemon that will not start until you write TOML is a daemon nobody runs on their
//! phone.

use offload_core::{AcceptWork, AgentKind, DeviceClass, PermissionMode, WorkPolicy};
use offload_workspace::UntrackedPolicy;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Human-readable name for this device. Cosmetic; `NodeId` is the real identity.
    pub name: String,
    /// Where mirrors, worktrees, run logs, and the node identity live.
    pub state_dir: PathBuf,
    /// Control socket. Defaults to `<state_dir>/offloadd.sock`.
    pub socket: Option<PathBuf>,
    /// Do this device's bytes cost money? `None` — the default — means ask the platform
    /// (ADR-0045).
    ///
    /// Nominated rather than probed for the reason `agent.max_concurrent` is: NetworkManager
    /// cannot see through a tether, so a laptop sharing a phone's connection looks like ordinary
    /// wifi from below and is guessed free. Whoever plugged it in knows. Saying nothing is not
    /// the same as saying `false` — that was the bug this replaced — so an unset value leaves the
    /// capability `Unknown` unless something on the machine can actually answer.
    #[serde(default)]
    pub metered: Option<offload_core::Metered>,
    pub agent: AgentConfig,
    pub checkpoint: CheckpointConfig,
    pub cluster: ClusterConfig,
    /// Where run checkouts go (ADR-0074).
    pub workspace: WorkspaceConfig,
    /// Owner policy overrides. `None` means "use the per-device-class default", which is
    /// almost always what you want — see `WorkPolicy::for_class`.
    pub policy: Option<PolicyConfig>,
    /// Routes from a run to a human (ADR-0010). Empty by default, which is honest: a device
    /// with no configured route can reach nobody, and claiming otherwise is the one failure
    /// this plane cannot detect on its own.
    #[serde(default)]
    pub sinks: Vec<SinkConfig>,
    /// Things on this device a run may be granted the use of (ADR-0011).
    #[serde(default)]
    pub resources: Vec<ResourceConfig>,
    /// Programs on this device that notice something happening (ADR-0011's `Role::Trigger`,
    /// ADR-0020). Empty by default, for the sink's reason: a device that watches nothing is
    /// the honest default, and claiming otherwise is the one failure this plane cannot detect.
    #[serde(default)]
    pub triggers: Vec<TriggerConfig>,
    /// Programs on this device a run may *be* (ADR-0019 §1). Empty by default, for the sink's
    /// reason: a device that offers no task is the honest default.
    #[serde(default)]
    pub tasks: Vec<TaskConfig>,
}

/// When to checkpoint, and what to carry.
///
/// ADR-0003 says a checkpoint's cadence is what decides how much a crash costs and leaves
/// the number open. It is one per turn here, deliberately:
///
/// * A turn boundary is the only safe point (ADR-0004), so the cadence can only ever be
///   *some multiple* of it. The question is which multiple, and one is the only value that
///   loses nothing.
/// * The cost is smaller than it looks. Blobs are content-addressed, so a bundle that has
///   not changed since the last turn is stored once, and the transcript blob a checkpoint
///   supersedes stops being referenced by any run the moment it is replaced — GC reclaims
///   it without being told anything special.
/// * The thing being protected is an agent turn: minutes of model time and real money. A
///   few hundred kilobytes of blob churn to avoid re-running one is not a close call.
///
/// Where a run's checkout is made (ADR-0074): the directory a person opens to read what an agent
/// did, which is why it is not the state directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// A directory of this node's own. `~/` means this user's home. Unset, `offloadd` uses
    /// `~/offload` on a desktop, and everything else (the phone apps, tests) `<state_dir>/worktrees`.
    pub dir: Option<PathBuf>,
}

/// `every_turns` exists for the case that argument does not cover — a very large repo on a
/// metered link, where writing a patch every turn is the expensive part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckpointConfig {
    /// Checkpoint every N turn boundaries. `0` disables automatic checkpoints entirely —
    /// an explicit `offload checkpoint` still works.
    pub every_turns: u32,
    /// Skip untracked files larger than this.
    pub max_untracked_file_bytes: u64,
    /// Stop selecting untracked files once they total this much.
    pub max_untracked_total_bytes: u64,
    /// Extra directory names to treat as build output, on top of the built-in list.
    pub never_source: Vec<String>,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        let untracked = UntrackedPolicy::default();
        CheckpointConfig {
            every_turns: 1,
            max_untracked_file_bytes: untracked.max_file_bytes,
            max_untracked_total_bytes: untracked.max_total_bytes,
            never_source: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    /// Path to the agent binary.
    pub binary: PathBuf,
    /// Model used when a run does not name one. `None` lets the agent pick its own
    /// default, which is usually right — pinning a model here means every run on this
    /// node silently disagrees with every run on another.
    pub default_model: Option<String>,
    /// Permission mode used when a run does not name one.
    ///
    /// `AcceptEdits` per ADR-0008: the worktree is disposable, so unrestricted editing
    /// inside it is a small grant, while anything else still gates.
    pub default_permission_mode: PermissionMode,
    /// How long a cancelled agent gets to exit before its process group is killed.
    pub cancel_grace_secs: u64,
    /// Tools every run on this node may use, e.g. `["Bash(git status:*)"]`.
    ///
    /// Operator-set, so it is not capped the way a repo's allowlist is — but a risky
    /// entry here is logged at startup, because "I allowlisted things" should not feel
    /// like a limit if one of the entries is a shell.
    pub allow: Vec<String>,
    /// How many sessions this install can sustain at once. Unset means the probe's conservative
    /// guess, which is 2.
    ///
    /// **The one number about an agent only its owner knows**, which is why it is nominated here
    /// rather than probed — the sink and resource rule, applied to the thing an agent *is*: what
    /// bounds concurrent sessions is a plan and a rate limit, and nothing on the machine says
    /// which one this login has.
    ///
    /// It matters because the guess is a **per-node cap that binds before the owner's own**:
    /// `WorkPolicy::max_concurrent_runs` defaults to 4 on a desktop, server or VM, and a third
    /// run there was refused with "agent claude-code is at its per-node concurrency limit" — a
    /// per-node limit no config could express, while `offload status` went on reporting 4 as the
    /// ceiling. Raising `max_concurrent_runs` did nothing at all.
    ///
    /// Still a *capability* and not a policy, and the split is worth keeping straight: this says
    /// what the install can do, `policy.max_concurrent_runs` says how much of the machine the
    /// owner will give it, and `policy.max_concurrent_account` says what the account may run
    /// fleet-wide. The lowest of the three is what happens.
    pub max_concurrent: Option<AgentConcurrency>,
    /// Where the agent keeps its state — which is what selects the **account** (ADR-0028).
    ///
    /// Unset means ask `$CLAUDE_CONFIG_DIR` and fall back to `~/.claude`, which is what this
    /// always did and is right for a machine with one login. Set it when the machine has more
    /// than one, or when the daemon does not run from the shell whose environment says so: a
    /// service manager's environment is not the owner's, and before this field there was nothing
    /// in `node.toml` that could say which account a device spends on.
    ///
    /// It reaches **three** places and has to reach all of them, because they answer different
    /// questions about the same directory: the probe (is this device authenticated, and as whom),
    /// the capture (where the transcript to checkpoint is), and the spawn (where the agent will
    /// look). Two out of three is the failure that hides — a node authenticated as the account the
    /// owner named, checkpointing nothing all night because the agent wrote its transcript
    /// somewhere else.
    pub config_dir: Option<PathBuf>,
    /// The account this device may use, as `offload probe` prints it (`acct:…`).
    ///
    /// A guard rather than a selector: what *selects* an account is [`Self::config_dir`], and this
    /// says which one the owner meant. If the resolved account is anything else — a different
    /// login, or none at all — the node advertises the agent as **not authenticated** and says
    /// why, so it refuses every run instead of quietly spending somebody's work account on
    /// personal repositories. Refusing loudly is `offload-probe`'s rule and this is the one
    /// mistake in this area that costs money.
    ///
    /// The fingerprint and not an email or a uuid, because that is the value already designed to
    /// be compared across machines and shown to people (`AccountId`) — and because a config file
    /// holding a login's real identifier is a small leak with no purpose.
    pub account: Option<String>,
    /// Honour `.offload.toml` from the repository being worked on.
    ///
    /// On by default and capped to scoped grants. Turn it off to ignore repo-declared
    /// allowlists entirely — appropriate when running agents on repos you do not control.
    pub trust_repo_allowlist: bool,
}

/// Talking to other nodes.
///
/// Off means off: a node with no fleet, or with `enabled = false`, is a fleet of one and
/// works exactly as it did in phases 1 and 2. That is not a degraded mode — it is the mode
/// most of this project's testing happens in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClusterConfig {
    pub enabled: bool,
    /// Where to listen for peers. Port 0 picks an ephemeral one, which is right for a second
    /// node on one machine and wrong for anything a seed list points at. `[::]` is both families;
    /// `0.0.0.0` is IPv4 only, and can then dial no IPv6 seed either.
    pub listen: String,
    /// Announce this node on the LAN, and dial the peers that announce themselves.
    ///
    /// On by default because a personal fleet on one network should need no configuration at
    /// all. Turn it off where multicast is pointless or unwelcome — a VPS, a guest network —
    /// and use `seeds` instead.
    pub mdns: bool,
    /// Addresses of nodes to dial on startup, `host:port`.
    ///
    /// Addresses rather than `<key>@host:port`, because a seed list is written by a human who
    /// knows where a node lives and not what its key is. The key is learned from the
    /// handshake, and membership is what decides whether the answer counts.
    pub seeds: Vec<String>,
    /// How often to probe one peer. Detection time is roughly this times the fleet size,
    /// since the rotation visits everybody.
    pub probe_interval_ms: u64,
    pub probe_timeout_ms: u64,
    /// How long a bid round waits for an answer, per peer.
    ///
    /// Somebody is standing at the keyboard while this happens (ADR-0014), so it is short: a
    /// node that has gone quiet costs the round this much and nothing more.
    pub bid_window_ms: u64,
    /// How long a drain waits for agents to reach a turn boundary.
    ///
    /// A turn is minutes of model time, and mid-turn is not a safe capture point (ADR-0004),
    /// so this is generous. What it bounds is the wait, not the turn: a run still mid-turn at
    /// the deadline is left to its lease rather than snapshotted anyway.
    pub drain_deadline_secs: u64,
    /// How long a node stays `Suspect` before it is called `Dead`. Long enough for it to hear
    /// the suspicion and refute it — that is what the state is for.
    pub suspect_timeout_ms: u64,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        ClusterConfig {
            enabled: true,
            // Unassigned by IANA, and configurable; nothing depends on the number. `[::]` rather
            // than `0.0.0.0`: the transport binds it dual-stack, so IPv4 peers reach it exactly as
            // before and an IPv6 one — a phone on mobile data — can too (ADR-0037 §0).
            listen: "[::]:7433".to_string(),
            mdns: true,
            seeds: Vec::new(),
            probe_interval_ms: 1_000,
            probe_timeout_ms: 500,
            bid_window_ms: 2_000,
            drain_deadline_secs: 300,
            suspect_timeout_ms: 5_000,
        }
    }
}

impl ClusterConfig {
    #[must_use]
    pub fn detector(&self) -> offload_cluster::DetectorConfig {
        offload_cluster::DetectorConfig {
            probe_interval: offload_core::Millis(self.probe_interval_ms),
            probe_timeout: offload_core::Millis(self.probe_timeout_ms),
            indirect_peers: 3,
            suspect_timeout: offload_core::Millis(self.suspect_timeout_ms),
        }
    }
}

/// One route from this device to a human (ADR-0010).
///
/// A **command the owner nominated**, because that is the only sink a general-purpose machine
/// can honestly offer today: there are no push credentials, no mail transport and no HTTP
/// client in this tree, and a route that cannot be verified must not be claimed. What a
/// two-line script can reach — `notify-send`, `ntfy`, `mail`, a webhook via `curl` — is
/// genuinely everything, and it puts the egress decision in the hands of the person whose
/// machine it is.
///
/// Three deliberate shapes:
///
/// * **`service` is what the owner says this route *is*** — `push`, `email`, `chat:team`,
///   `webhook` — and the command is only how it is invoked. That separation is what lets a
///   future notification ask for "reach me by push" as a `Constraint` (ADR-0011's
///   `HasService`), instead of every route on every device being indistinguishable.
/// * **No templating.** The notification arrives as JSON on stdin, as `OFFLOAD_*` environment
///   variables, and as one final argument holding the summary line. A little template language
///   would be a quoting bug with a syntax, and the tool-allowlist lesson applies: something
///   that looks scoped and is not is worse than nothing, because it is trusted.
/// * **Nothing here is gossiped.** The command, its arguments and its environment stay on this
///   node. A peer learns that this device *has* a push route, never how it works — the same
///   rule agent credentials follow, and the reason a sink's credentials never migrate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SinkConfig {
    /// Short, stable, and this node's own. It becomes the capability id (`sink:<id>`) and the
    /// dedup key in the outbox, so renaming one is retiring it and adding another — which is
    /// the same rule `CapabilityId` already states.
    pub id: String,
    /// What kind of route this is, spelled as `offload_core::Service` displays it.
    pub service: String,
    /// The program to run. Absolute, or resolved on `PATH` at probe time.
    pub command: String,
    /// Fixed arguments. The notification's summary is appended after these.
    pub args: Vec<String>,
    /// Who this route reaches, if it is worth saying. Comparable across nodes, which is how
    /// two devices can be recognised as one mailbox.
    pub identity: Option<String>,
    /// One line for `offload sinks`. Never matched on.
    pub description: String,
}

impl SinkConfig {
    /// The capability id this route advertises itself under.
    #[must_use]
    pub fn capability_id(&self) -> String {
        format!("sink:{}", self.id)
    }

    /// What it claims to be, or why that cannot be read.
    pub fn service(&self) -> Result<offload_core::Service, ConfigError> {
        self.service
            .parse()
            .map_err(|e: offload_core::UnknownService| ConfigError::Parse(e.to_string()))
    }
}

/// One thing on this device that a run may be granted the use of (ADR-0011, `Role::Resource`).
///
/// An **MCP server the owner nominated**, for the reason a sink is a command they nominated:
/// there is no mail client, no calendar client and no credential store in this tree, and a
/// resource that cannot be verified must not be claimed. What the owner *can* verify is that a
/// program exists, and the agent already has a protocol for talking to one.
///
/// The shapes, and each is the sink rule applied to the other direction:
///
/// * **`service` is what the owner says this is** — `email`, `calendar`, anything — and the
///   command is only how it is invoked. That separation is what lets a run ask to reach email
///   without naming a script on a particular machine.
/// * **`access` describes the grant, and does not select it.** The owner knows whether their
///   integration reads, writes or both; the person submitting a run knows only that it needs
///   email. Making the run name an access it cannot know would have it guess, and a guess too
///   narrow is a run that will not place while the resource sits there unused.
/// * **Nothing here is gossiped.** A peer learns this device offers `email` for a run to use,
///   never the command, its arguments or its environment — the rule every credential in this
///   design follows, and the reason a resource on another node will one day be a *proxied call*
///   rather than a shipped secret.
///
/// A run reaches this only if it was granted it by name. Holding the resource and hosting the
/// run are two different things (`--use`), which is the whole of what makes `Role::Resource`
/// different from the other three.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceConfig {
    /// Short, stable, and this node's own. It becomes the capability id (`resource:<id>`) and
    /// the MCP server's name in the config handed to the agent.
    pub id: String,
    /// What the owner says this is, spelled as `offload_core::Service` displays it.
    pub service: String,
    /// The program to run. Absolute, or resolved on `PATH` at probe time.
    pub command: String,
    pub args: Vec<String>,
    /// Environment for the server process. Here rather than inherited, because inheriting the
    /// daemon's environment is the ambient-authority mistake this whole area exists to avoid.
    pub env: Vec<(String, String)>,
    /// `read`, `write` or `read-write`. Unset is `read`, the conservative half.
    pub access: Option<String>,
    /// Who it acts as, if it is worth saying. Comparable across nodes.
    pub identity: Option<String>,
    /// One line for `offload probe`. Never matched on.
    pub description: String,
}

impl ResourceConfig {
    #[must_use]
    pub fn capability_id(&self) -> String {
        format!("resource:{}", self.id)
    }

    pub fn service(&self) -> Result<offload_core::Service, ConfigError> {
        self.service
            .parse()
            .map_err(|e: offload_core::UnknownService| ConfigError::Parse(e.to_string()))
    }

    /// What the owner said this can do, defaulting to the conservative half.
    pub fn access(&self) -> Result<offload_core::Access, ConfigError> {
        match self.access.as_deref().unwrap_or("read").trim() {
            "read" => Ok(offload_core::Access::Read),
            "write" => Ok(offload_core::Access::Write),
            "read-write" | "readwrite" | "rw" => Ok(offload_core::Access::ReadWrite),
            other => Err(ConfigError::Parse(format!(
                "unknown access `{other}` (expected: read, write, read-write)"
            ))),
        }
    }
}

/// One thing on this device that notices something happening (ADR-0011's `Role::Trigger`,
/// settled by ADR-0020).
///
/// A **long-lived program the owner nominated**, which is the sink rule and the resource rule
/// applied a third time and for the third direction. The one thing a general-purpose machine can
/// honestly verify is that a program exists; what it watches, and how often, is the owner's
/// business and never the daemon's.
///
/// The protocol is one line of its stdout per event, and that is the whole of it:
///
/// * **The cadence is the program's.** There is deliberately no `interval` field. A daemon that
///   polled on a schedule would be a scheduler inside an orchestrator — "never reimplement an
///   agent" broken from the same end ADR-0019 rejected a built-in HTTP poller from — and it
///   would hand the daemon an opinion about missed ticks it otherwise never needs. Something
///   that fires on a clock is `sh -c 'while sleep 300; do echo tick; done'`, nominated like
///   anything else, which is what makes `Service::Schedule` mean something without a cron parser.
/// * **stderr is this node's own log**, tagged with the trigger id, because a watcher's
///   complaints are for the person whose machine it is.
/// * **A program that keeps dying is retried for ever and reported.** Never given up on: the
///   delivery plane's lesson (a route that stopped being retried is not a route) read in the
///   inbound direction, where the failure would be a watcher silently ceasing to watch.
/// * **Nothing here is gossiped.** A peer learns this device has an `email` trigger and never
///   how it watches — the rule every credential in this design follows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TriggerConfig {
    /// Short, stable, and this node's own. It becomes the capability id (`trigger:<id>`) and
    /// what `offload triggers` reports against, so renaming one retires it and adds another.
    pub id: String,
    /// What the owner says fires here, spelled as `offload_core::Service` displays it. A rule
    /// names this and never the id, for the reason an `Audience` does: an id is a node's name
    /// for one of its own things, and naming one would pin a standing instruction to a device.
    pub service: String,
    /// The program to run. Absolute, or resolved on `PATH` at probe time.
    pub command: String,
    pub args: Vec<String>,
    /// Environment for the watcher. Here rather than inherited, because inheriting the daemon's
    /// environment is the ambient-authority mistake this whole area exists to avoid.
    pub env: Vec<(String, String)>,
    /// What it watches on behalf of, if it is worth saying. Comparable across nodes.
    pub identity: Option<String>,
    /// One line for `offload triggers`. Never matched on.
    pub description: String,
}

impl TriggerConfig {
    #[must_use]
    pub fn capability_id(&self) -> String {
        format!("trigger:{}", self.id)
    }

    pub fn service(&self) -> Result<offload_core::Service, ConfigError> {
        self.service
            .parse()
            .map_err(|e: offload_core::UnknownService| ConfigError::Parse(e.to_string()))
    }
}

/// A program this device's owner nominated as work a run may *be* (ADR-0019 §1).
///
/// The third application of one rule, after sinks (ADR-0010) and resources (ADR-0011), with the
/// same justification each time: **the service is the owner's declaration and the command is
/// only how it is invoked.** A run asks for `watch-api` by service and never by command, the
/// command never leaves this node, and a peer learns that this device *has* such a task without
/// learning what it runs.
///
/// Two things follow, and they are the reason this is the design rather than a convenience.
///
/// * **The outbound access is the owner's grant, not the submitter's.** Nominating the program
///   *is* the grant. There is no allowlist story for a task's network access because the
///   submitter never says where to connect — a gap that closes by choosing the right shape
///   rather than by adding a mechanism.
/// * **A submitter cannot execute arbitrary code anywhere in the fleet.** A submitter-supplied
///   command line is refused by construction, because there is nowhere to put one. That is the
///   allowlist rule read together with the sink rule: a repo may say "run my tests", never
///   "give me `sh`".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskConfig {
    /// Short, stable, and this node's own. It becomes the capability id (`task:<id>`).
    pub id: String,
    /// What the owner says this task *is*, spelled as `offload_core::Service` displays it.
    /// This is what a submitter names, and the only part of this entry that is gossiped.
    pub service: String,
    /// The program to run. Absolute, or resolved on `PATH` at probe time.
    pub command: String,
    /// Fixed arguments. A run's own `args` are appended after these, never in front of them —
    /// so the owner's arguments cannot be displaced by the submission.
    pub args: Vec<String>,
    /// Environment for the task process. Here rather than inherited, for `ResourceConfig`'s
    /// reason: inheriting the daemon's environment is the ambient-authority mistake this whole
    /// area exists to avoid.
    pub env: Vec<(String, String)>,
    /// One line for `offload probe`. Never matched on.
    pub description: String,
}

impl TaskConfig {
    /// The capability id this task advertises itself under.
    #[must_use]
    pub fn capability_id(&self) -> String {
        format!("task:{}", self.id)
    }

    /// What it claims to be, or why that cannot be read.
    pub fn service(&self) -> Result<offload_core::Service, ConfigError> {
        self.service
            .parse()
            .map_err(|e: offload_core::UnknownService| ConfigError::Parse(e.to_string()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyConfig {
    pub accept: Option<AcceptWork>,
    pub max_concurrent_runs: Option<u32>,
    /// Shares this device will have committed at once (ADR-0013). Unset means
    /// `max_concurrent_runs` ordinary runs' worth, which is what every config said before
    /// there was a budget — so saying nothing keeps exactly the old behaviour.
    pub budget_shares: Option<u32>,
    /// Runs this device will have going on one agent *account* at once, fleet-wide. Unset
    /// means no ceiling — nobody here knows what an account's real rate limit is, so the
    /// owner is the only one who can say (ADR-0013).
    pub max_concurrent_account: Option<u32>,
    /// The largest workspace archive this device will take, in bytes (ADR-0061 §4).
    ///
    /// A *tightening* knob. Unset means the fleet's own limit and no opinion beyond it — never
    /// "no limit", because one has always existed. A value **above** the fleet's is refused
    /// rather than clamped: an owner who wrote a bigger number believes something about their
    /// fleet that is not true, and clamping silently would leave them believing it.
    pub max_archive_bytes: Option<u64>,
    pub min_battery_percent: Option<u8>,
    pub allow_metered: Option<bool>,
    /// Which agents this device will host, by name. Unset means any agent it has.
    ///
    /// The owner's answer to "what *kind* of work is this machine for", which is the one policy
    /// knob a device class cannot express: a phone whose owner is happy to hold a light workload
    /// and not an expensive agent session is stating a preference, not lacking a capability.
    ///
    /// It was on [`WorkPolicy`] and enforced by `admits` from the beginning, with nothing able to
    /// set it — so `Refusal::AgentNotAllowed` was a sentence no fleet could produce. A policy rule
    /// that cannot be reached is worse than one that does not exist, because it reads as a
    /// control that is being applied.
    pub allowed_agents: Option<AllowedAgents>,
    /// The same three standing doors, for light work only (ADR-0019 §4). `[policy.light]`.
    ///
    /// Every field inside it is an override with "say nothing and nothing changes" as its
    /// default, which is the whole shape of this feature: these are gates people have already
    /// configured, and a light form that silently tightened or loosened one would be worse
    /// than not having it. The sentence it exists for is *take the light watcher always, heavy
    /// work only while charging* — `accept = "when_charging"` up here and
    /// `[policy.light] accept = "always"` below it.
    #[serde(default)]
    pub light: Option<LightPolicyConfig>,
}

/// `[policy.light]` — what the owner permits for `Demand::Light` work (ADR-0019 §4).
///
/// A separate struct rather than three `light_*` keys on [`PolicyConfig`] so the table reads as
/// what it is: one alternative set of the same three doors, and `deny_unknown_fields` catches
/// `[policy.light] max_concurrent_runs = 4` — a knob that does not exist here, because capacity
/// and the budget already consult demand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LightPolicyConfig {
    pub accept: Option<AcceptWork>,
    /// `0` is how to say *any* charge will do; leaving it out inherits the main floor.
    pub min_battery_percent: Option<u8>,
    pub allow_metered: Option<bool>,
}

/// The agents an owner will host here, parsed as the config is read.
///
/// A newtype with its own `Deserialize` rather than a `Vec<String>` validated later, and that is
/// the whole point: [`Config::work_policy`] is called from the gossip tick and from every status
/// answer, so it must not be able to fail — and a fallible version's only safe fallback would be
/// "the owner said nothing", which is *less* restrictive than what they wrote. Refusing here means
/// an invalid name never becomes a `WorkPolicy` at all, and the daemon says so at startup where
/// somebody is watching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllowedAgents(pub std::collections::BTreeSet<AgentKind>);

impl<'de> Deserialize<'de> for AllowedAgents {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(d)?;
        if names.is_empty() {
            return Err(serde::de::Error::custom(
                "policy.allowed_agents is empty, which would refuse every run. Say \
                 `accept = \"never\"` if this device should host nothing",
            ));
        }
        names
            .iter()
            .map(|name| match name.trim() {
                // Refused rather than accepted as `AgentKind::Other`, for the reason
                // `Service::from_str` gives in the capability direction: a name this build cannot
                // spawn is a claim it must not make. A typo accepted here would leave the daemon
                // starting happily and refusing every run at 03:00 with "agent claude-code is not
                // allowed by policy" — true, legible, and about a line the owner believes says the
                // opposite.
                "claude-code" | "claude" => Ok(AgentKind::ClaudeCode),
                other => Err(serde::de::Error::custom(format!(
                    "policy.allowed_agents names `{other}`, which this build cannot spawn \
                     (known: claude-code)"
                ))),
            })
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map(AllowedAgents)
    }
}

/// How many concurrent sessions the owner says this install sustains.
///
/// A newtype with its own `Deserialize` for [`AllowedAgents`]' reason: the capability set is
/// rebuilt on the gossip tick and at every status answer, so it must not be able to fail there,
/// and the only safe fallback for a bad value would be the *default* — which is not what the
/// owner wrote and, being 2, is not even conservative relative to a larger number they meant.
///
/// Zero is refused rather than taken at its word. It reads as a limit and means "host nothing",
/// which is `accept = "never"` said in a way no other part of this understands — the same
/// sentence `policy.allowed_agents = []` earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentConcurrency(pub u32);

impl<'de> Deserialize<'de> for AgentConcurrency {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let n = u32::deserialize(d)?;
        if n == 0 {
            return Err(serde::de::Error::custom(
                "agent.max_concurrent is 0, which would host nothing. Say \
                 `accept = \"never\"` under [policy] if that is what you mean",
            ));
        }
        Ok(AgentConcurrency(n))
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            name: hostname(),
            state_dir: default_state_dir(),
            socket: None,
            metered: None,
            agent: AgentConfig::default(),
            checkpoint: CheckpointConfig::default(),
            cluster: ClusterConfig::default(),
            workspace: WorkspaceConfig::default(),
            policy: None,
            sinks: Vec::new(),
            resources: Vec::new(),
            tasks: Vec::new(),
            triggers: Vec::new(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            binary: PathBuf::from("claude"),
            default_model: None,
            // ADR-0008. Not `Ask`: headless there is nobody to answer, so an `Ask` run
            // just accumulates denials.
            default_permission_mode: PermissionMode::AcceptEdits,
            cancel_grace_secs: 10,
            allow: Vec::new(),
            max_concurrent: None,
            config_dir: None,
            account: None,
            trust_repo_allowlist: true,
        }
    }
}

impl Config {
    /// Load from a TOML file, or return defaults if the path is `None`.
    /// Where run checkouts go: the configured directory, or `<state_dir>/worktrees`.
    #[must_use]
    pub fn checkouts_dir(&self) -> PathBuf {
        match &self.workspace.dir {
            Some(dir) => expand_home(dir, std::env::var_os("HOME").map(PathBuf::from).as_deref()),
            None => self.state_dir.join("worktrees"),
        }
    }

    /// Fill in the checkouts directory a person reads, `~/offload`, when the config names none
    /// (ADR-0074). Only `offloadd` calls this, at start: a default that applied wherever a
    /// `Config` is built would put every test's checkouts in its author's home.
    #[must_use]
    pub fn with_readable_checkouts(mut self, home: Option<&Path>) -> Self {
        if self.workspace.dir.is_none() {
            self.workspace.dir = home.map(|home| home.join("offload"));
        }
        self
    }

    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let Some(path) = path else {
            return Ok(Config::default());
        };
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Read {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.check()?;
        Ok(config)
    }

    /// What the parser cannot say: values that are legal TOML and wrong here.
    fn check(&self) -> Result<(), ConfigError> {
        // ADR-0061 §4. `max_archive_bytes` tightens the fleet's own limit and may not loosen it,
        // so a larger number is refused rather than clamped — an owner who wrote one believes
        // their fleet will move that much, and silently clamping leaves them believing it until
        // a run fails somewhere else. Both numbers, because the useful sentence is the
        // difference between them.
        let cap = offload_proto::cluster::MAX_BLOB_BYTES;
        if let Some(limit) = self.policy.as_ref().and_then(|p| p.max_archive_bytes) {
            if limit > cap {
                return Err(ConfigError::Parse(format!(
                    "policy.max_archive_bytes is {limit}, which is above the {cap} bytes this \
                     fleet can move in one exchange — it tightens that limit and cannot raise \
                     it, so there is no setting here that would make a larger archive travel"
                )));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.socket
            .clone()
            .unwrap_or_else(|| self.state_dir.join("offloadd.sock"))
    }

    /// This node's baseline allowlist.
    pub fn node_allowlist(&self) -> Result<offload_core::ToolAllowlist, ConfigError> {
        offload_core::ToolAllowlist::parse(&self.agent.allow)
            .map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// What a checkpoint carries out of the worktree, per this node's config.
    #[must_use]
    pub fn untracked_policy(&self) -> UntrackedPolicy {
        UntrackedPolicy {
            max_file_bytes: self.checkpoint.max_untracked_file_bytes,
            max_total_bytes: self.checkpoint.max_untracked_total_bytes,
            also_never_source: self.checkpoint.never_source.clone(),
        }
    }

    #[must_use]
    pub fn runs_dir(&self) -> PathBuf {
        self.state_dir.join("runs")
    }

    /// Where this node's keypair lives. Owner-readable only — it is a private key now, not
    /// the public value the pre-phase-3 `node-id` file held.
    #[must_use]
    pub fn identity_path(&self) -> PathBuf {
        self.state_dir.join(crate::identity::KEY_FILE)
    }

    /// Build the effective work policy: the class default, with configured overrides.
    ///
    /// Starting from the class default rather than from a blank struct means a phone
    /// with an empty `[policy]` block still gets phone-appropriate behaviour instead of
    /// whatever zero happens to mean.
    #[must_use]
    pub fn work_policy(&self, class: DeviceClass) -> WorkPolicy {
        let mut policy = WorkPolicy::for_class(class);
        let Some(over) = &self.policy else {
            return policy;
        };
        if let Some(accept) = over.accept {
            policy.accept = accept;
        }
        if let Some(max) = over.max_concurrent_runs {
            policy.max_concurrent_runs = max;
        }
        if over.budget_shares.is_some() {
            policy.budget_shares = over.budget_shares;
        }
        if over.max_concurrent_account.is_some() {
            policy.max_concurrent_account = over.max_concurrent_account;
        }
        if over.max_archive_bytes.is_some() {
            policy.max_archive_bytes = over.max_archive_bytes;
        }
        if over.min_battery_percent.is_some() {
            policy.min_battery_percent = over.min_battery_percent;
        }
        if let Some(metered) = over.allow_metered {
            policy.allow_metered = metered;
        }
        if let Some(agents) = &over.allowed_agents {
            policy.allowed_agents = Some(agents.0.clone());
        }
        // Copied wholesale rather than field by field, because every field is already an
        // override of the block above and `None` already means inherit. There is nothing for
        // this layering to decide.
        if let Some(light) = over.light {
            policy.light = offload_core::LightWork {
                accept: light.accept,
                min_battery_percent: light.min_battery_percent,
                allow_metered: light.allow_metered,
            };
        }
        policy
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {reason}")]
    Read { path: PathBuf, reason: String },
    #[error("invalid config: {0}")]
    Parse(String),
}

/// Where `offloadd` and the local `offload` commands look for a config file when none is named
/// (ADR-0074): `$XDG_CONFIG_HOME/offload/node.toml`, else `~/.config/offload/node.toml`. Used only
/// if it exists, since every setting has a working default.
#[must_use]
pub fn default_config_path(
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    xdg_config_home
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))
        .map(|dir| dir.join("offload").join("node.toml"))
}

/// The config file to read: the one named, else the default one if it is there.
#[must_use]
pub fn config_path(named: Option<&Path>) -> Option<PathBuf> {
    named.map(Path::to_path_buf).or_else(|| {
        default_config_path(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
        .filter(|path| path.is_file())
    })
}

/// `~` and `~/…` against `home`; anything else as written.
fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("OFFLOAD_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".offload")))
        .unwrap_or_else(|| PathBuf::from("/tmp/offload"))
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unnamed".to_string())
}

#[cfg(test)]
mod tests {
    /// ADR-0074: config under `~/.config/offload`, checkouts in `~/offload`, and neither a
    /// default for a `Config` built anywhere but `offloadd`, so tests keep their scratch dirs.
    #[test]
    fn config_lives_in_dot_config_and_checkouts_in_offload() {
        let home = PathBuf::from("/home/o");
        assert_eq!(
            default_config_path(None, Some(home.clone())),
            Some(PathBuf::from("/home/o/.config/offload/node.toml"))
        );
        assert_eq!(
            default_config_path(Some("/x/cfg".into()), Some(home.clone())),
            Some(PathBuf::from("/x/cfg/offload/node.toml"))
        );
        // A relative XDG_CONFIG_HOME is invalid by the spec, and ignored.
        assert_eq!(
            default_config_path(Some("cfg".into()), Some(home.clone())),
            Some(PathBuf::from("/home/o/.config/offload/node.toml"))
        );

        let plain = Config {
            state_dir: "/s".into(),
            ..Config::default()
        };
        assert_eq!(plain.checkouts_dir(), PathBuf::from("/s/worktrees"));
        let started = plain.clone().with_readable_checkouts(Some(&home));
        assert_eq!(
            started.workspace.dir,
            Some(PathBuf::from("/home/o/offload"))
        );

        let named = Config::parse("[workspace]\ndir = \"/data/runs\"\n").expect("parses");
        assert_eq!(named.checkouts_dir(), PathBuf::from("/data/runs"));
        assert_eq!(
            named.with_readable_checkouts(Some(&home)).checkouts_dir(),
            PathBuf::from("/data/runs"),
            "a configured directory wins"
        );
        assert_eq!(
            expand_home(Path::new("~/runs"), Some(&home)),
            PathBuf::from("/home/o/runs")
        );
        assert_eq!(expand_home(Path::new("~"), Some(&home)), home);
        assert_eq!(
            expand_home(Path::new("/abs"), Some(&home)),
            PathBuf::from("/abs")
        );
    }

    use super::*;

    #[test]
    fn a_policy_that_tries_to_raise_the_fleets_archive_limit_is_refused() {
        // ADR-0061 §4: `max_archive_bytes` tightens and never loosens. Refused rather than
        // clamped, because an owner who wrote a bigger number believes their fleet will move
        // that much — and clamping silently leaves them believing it until a run fails
        // somewhere else, which is the report defect this tree keeps paying for.
        let cap = offload_proto::cluster::MAX_BLOB_BYTES;
        let err = Config::parse(&format!(
            "name = \"a\"\n[policy]\nmax_archive_bytes = {}\n",
            cap + 1
        ))
        .expect_err("a larger limit is refused");
        let text = err.to_string();
        assert!(text.contains(&(cap + 1).to_string()), "{text}");
        assert!(
            text.contains(&cap.to_string()),
            "names both numbers: {text}"
        );

        // Tightening is the point of the knob, so it is accepted.
        let smaller = Config::parse(&format!(
            "name = \"a\"\n[policy]\nmax_archive_bytes = {}\n",
            cap / 2
        ))
        .expect("tightening is allowed");
        assert_eq!(
            smaller.work_policy(DeviceClass::Laptop).max_archive_bytes,
            Some(cap / 2),
            "and it reaches the policy"
        );

        // Exactly the cap is not a raise.
        assert!(Config::parse(&format!(
            "name = \"a\"\n[policy]\nmax_archive_bytes = {cap}\n"
        ))
        .is_ok());
    }

    #[test]
    fn an_empty_config_is_a_working_config() {
        // `offloadd` with no config file must still start. Defaults are the common path,
        // not a fallback.
        let cfg = Config::parse("").expect("empty config parses");
        assert_eq!(cfg.agent.binary, PathBuf::from("claude"));
        assert_eq!(
            cfg.agent.default_permission_mode,
            PermissionMode::AcceptEdits
        );
        assert!(cfg.socket_path().ends_with("offloadd.sock"));
    }

    #[test]
    fn the_default_permission_mode_is_the_one_adr_0008_chose() {
        // Guards the decision against a well-meaning "make the default safer" edit that
        // would leave every headless run unable to touch a file.
        assert_eq!(
            AgentConfig::default().default_permission_mode,
            PermissionMode::AcceptEdits
        );
        // The *type* default stays conservative; only the product default is relaxed.
        assert_eq!(PermissionMode::default(), PermissionMode::Ask);
    }

    #[test]
    fn parses_a_realistic_file() {
        let cfg = Config::parse(
            r#"
name = "desktop"
state_dir = "/var/lib/offload"
metered = "yes"

[agent]
default_model = "claude-opus-5"
default_permission_mode = "full"
cancel_grace_secs = 30

[policy]
max_concurrent_runs = 4
allow_metered = true
"#,
        )
        .expect("parse");

        assert_eq!(cfg.name, "desktop");
        assert_eq!(cfg.state_dir, PathBuf::from("/var/lib/offload"));
        // Nominated, and it has to survive the round trip: saying nothing leaves the capability
        // `Unknown` rather than `No`, which is the whole of ADR-0045.
        assert_eq!(cfg.metered, Some(offload_core::Metered::Yes));
        assert_eq!(
            Config::parse("name = \"x\"").expect("bare").metered,
            None,
            "an unset key is not a claim that the link is free"
        );
        assert_eq!(cfg.agent.default_model.as_deref(), Some("claude-opus-5"));
        assert_eq!(cfg.agent.default_permission_mode, PermissionMode::Full);
        assert_eq!(cfg.agent.cancel_grace_secs, 30);
        assert_eq!(
            cfg.socket_path(),
            PathBuf::from("/var/lib/offload/offloadd.sock")
        );
    }

    #[test]
    fn a_node_allowlist_parses_and_is_not_capped() {
        // Operator-set, unlike a repo's: the operator may grant a shell if they mean to.
        // It is logged, not refused.
        let cfg = Config::parse("[agent]\nallow = [\"Bash(cargo test:*)\", \"Bash(sh:*)\"]\n")
            .expect("parse");
        let allow = cfg.node_allowlist().expect("valid patterns");
        assert_eq!(allow.patterns().len(), 2);
        assert_eq!(allow.risky().len(), 1, "the shell is flagged, not rejected");
    }

    #[test]
    fn repo_allowlists_are_trusted_by_default_but_can_be_switched_off() {
        assert!(Config::default().agent.trust_repo_allowlist);
        let cfg = Config::parse("[agent]\ntrust_repo_allowlist = false\n").expect("parse");
        assert!(!cfg.agent.trust_repo_allowlist);
    }

    #[test]
    fn a_typo_in_the_config_is_an_error_not_a_shrug() {
        // `deny_unknown_fields`, because a silently ignored `max_concurent_runs` is a
        // node quietly running the wrong policy.
        let err = Config::parse("max_concurent_runs = 4").expect_err("should reject typo");
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn the_default_cadence_is_one_checkpoint_per_turn() {
        // ADR-0003's "a crash costs at most one turn" only holds at this value. Anything
        // higher is a deliberate trade the operator makes, not a default they inherit.
        assert_eq!(CheckpointConfig::default().every_turns, 1);

        let cfg = Config::parse("[checkpoint]\nevery_turns = 4\nnever_source = [\"fixtures\"]\n")
            .expect("parse");
        assert_eq!(cfg.checkpoint.every_turns, 4);
        assert_eq!(
            cfg.untracked_policy().also_never_source,
            vec!["fixtures".to_string()],
            "the untracked policy is built from config, not hardcoded"
        );
        assert_eq!(
            cfg.untracked_policy().max_file_bytes,
            UntrackedPolicy::default().max_file_bytes,
            "unset limits keep the module's own defaults"
        );
    }

    #[test]
    fn policy_overrides_layer_onto_the_class_default() {
        // A phone with an empty [policy] must still behave like a phone.
        let cfg = Config::parse("[policy]\nmax_concurrent_runs = 3\n").expect("parse");
        let policy = cfg.work_policy(DeviceClass::Phone);

        assert_eq!(policy.max_concurrent_runs, 3, "override applied");
        assert_eq!(
            policy.accept,
            AcceptWork::WhenCharging,
            "phone default preserved"
        );
        assert_eq!(policy.min_battery_percent, Some(40));
    }

    #[test]
    fn the_light_block_layers_on_and_says_nothing_when_it_is_absent() {
        // The sentence ADR-0019 §4 exists for, written the way an owner writes it.
        let cfg = Config::parse(
            r#"
[policy]
accept = "when_charging"

[policy.light]
accept = "always"
min_battery_percent = 0
"#,
        )
        .expect("parse");
        let policy = cfg.work_policy(DeviceClass::Phone);

        assert_eq!(policy.accept, AcceptWork::WhenCharging);
        assert_eq!(
            policy.accept_for(offload_core::Demand::Light),
            AcceptWork::Always
        );
        assert_eq!(
            policy.battery_floor_for(offload_core::Demand::Light),
            None,
            "a stated zero is any charge"
        );
        assert_eq!(
            policy.battery_floor_for(offload_core::Demand::Normal),
            Some(40),
            "the phone's own floor is untouched"
        );
        assert_eq!(
            policy.allows_metered_for(offload_core::Demand::Light),
            policy.allow_metered,
            "an unstated override inherits rather than defaulting to false"
        );

        // And the promise to every config written before this existed.
        let quiet = Config::parse("[policy]\naccept = \"when_charging\"\n").expect("parse");
        assert!(!quiet.work_policy(DeviceClass::Phone).light.is_stated());
        assert_eq!(
            quiet
                .work_policy(DeviceClass::Phone)
                .accept_for(offload_core::Demand::Light),
            AcceptWork::WhenCharging
        );

        // A knob that does not belong here is refused where somebody is watching, rather than
        // read as a light-work concurrency cap that nothing enforces.
        assert!(Config::parse("[policy.light]\nmax_concurrent_runs = 4\n").is_err());
    }

    #[test]
    fn an_account_ceiling_is_the_owners_to_set_and_unset_by_default() {
        // Nothing in the fleet can discover an account's real rate limit, so the only honest
        // default is no opinion — and the only way to get one is the owner saying so.
        let bare = Config::parse("[policy]\nmax_concurrent_runs = 3\n").expect("parse");
        assert_eq!(
            bare.work_policy(DeviceClass::Desktop)
                .max_concurrent_account,
            None
        );

        let capped = Config::parse("[policy]\nmax_concurrent_account = 4\n").expect("parse");
        assert_eq!(
            capped
                .work_policy(DeviceClass::Desktop)
                .max_concurrent_account,
            Some(4)
        );
    }

    #[test]
    fn which_agents_this_device_hosts_is_the_owners_to_say() {
        // `WorkPolicy::allowed_agents` was enforced by `admits` from the beginning and settable
        // by nothing — no config field, no flag — so `Refusal::AgentNotAllowed` was a sentence no
        // fleet could produce. It is the one owner decision a device *class* cannot express:
        // "this machine is for light work, not an expensive agent session" is a preference, not a
        // missing capability, which is exactly the split ADR-0013 rests on.
        let cfg = Config::parse("[policy]\nallowed_agents = [\"claude-code\"]\n").expect("parse");
        let policy = cfg.work_policy(DeviceClass::Phone);
        assert_eq!(
            policy.allowed_agents,
            Some([AgentKind::ClaudeCode].into_iter().collect())
        );
        assert_eq!(
            policy.accept,
            AcceptWork::WhenCharging,
            "and the rest of the phone's default is untouched"
        );

        // Unset stays unset, which means "any agent this node has" rather than "none".
        assert_eq!(
            Config::parse("[policy]\nmax_concurrent_runs = 1\n")
                .expect("parse")
                .work_policy(DeviceClass::Phone)
                .allowed_agents,
            None
        );
    }

    #[test]
    fn how_many_sessions_the_install_sustains_is_the_owners_to_say() {
        // The probe guesses 2, and the guess is a *cap*: on a desktop, where the owner's own
        // `max_concurrent_runs` defaults to 4, the third run was refused with "agent claude-code
        // is at its per-node concurrency limit" — a per-node limit no config could express, so
        // raising `max_concurrent_runs` did nothing at all above 2.
        let cfg = Config::parse("[agent]\nmax_concurrent = 5\n").expect("parse");
        assert_eq!(cfg.agent.max_concurrent, Some(AgentConcurrency(5)));
        // Unset is the probe's conservative guess, which is what every config said before this
        // field existed.
        assert_eq!(Config::default().agent.max_concurrent, None);

        // Zero reads as a limit and means "host nothing", which is `accept = "never"` said in a
        // way nothing else here understands — the sentence an empty `allowed_agents` earns.
        let zero = Config::parse("[agent]\nmax_concurrent = 0\n").expect_err("zero is not a limit");
        assert!(
            zero.to_string().contains("never"),
            "and it has to point at the thing that does mean this: {zero}"
        );
    }

    #[test]
    fn an_agent_this_build_cannot_spawn_is_refused_where_somebody_is_watching() {
        // At parse time, not at bid time, and not as `AgentKind::Other`. A typo accepted here
        // would let the daemon start happily and then refuse every run with "agent claude-code is
        // not allowed by policy" — a true sentence about a config line the owner believes says the
        // opposite. Same rule as `Service::from_str`'s in the capability direction: a name this
        // build cannot spawn is a claim it must not make.
        let err = Config::parse("[policy]\nallowed_agents = [\"claude-cod\"]\n")
            .expect_err("a name this build cannot spawn");
        assert!(
            err.to_string().contains("claude-cod"),
            "the refusal has to quote what was written: {err}"
        );

        // And an empty list, which reads as a restriction and means "refuse everything". The
        // device-level way to say that already exists and is understood everywhere.
        let empty =
            Config::parse("[policy]\nallowed_agents = []\n").expect_err("empty is not a policy");
        assert!(
            empty.to_string().contains("never"),
            "and it has to point at the thing that does mean this: {empty}"
        );
    }

    #[test]
    fn no_policy_block_means_pure_class_defaults() {
        let cfg = Config::parse("name = \"phone\"").expect("parse");
        assert_eq!(
            cfg.work_policy(DeviceClass::Phone),
            WorkPolicy::for_class(DeviceClass::Phone)
        );
    }
}
