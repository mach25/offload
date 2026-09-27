//! Driving coding agents as external processes.
//!
//! Offload does not implement an agent. It spawns one, reads its event stream, and knows
//! how to resume it — see `docs/adr/0004-agent-adapters.md` for why that boundary is worth
//! defending. Everything agent-specific (CLI flags, event schema, where the transcript
//! lives) is confined to an adapter module; nothing above this crate should know that
//! Claude Code takes a `--resume` flag.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod claude;
pub mod event;
pub mod transcript;
pub mod usage;

pub use event::{AgentEvent, Outcome, TurnTracker};

use offload_core::{AgentKind, PermissionMode, RunId, ToolAllowlist};
use std::path::PathBuf;

/// A session identifier. We mint it rather than scraping it from the agent's output —
/// Claude Code accepts `--session-id`, which makes the transcript locatable by a key we
/// chose instead of one we had to parse out of a stream.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    /// Mint a fresh session id. UUIDv4 because the agent validates the format.
    #[must_use]
    pub fn generate() -> Self {
        SessionId(uuid::Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything needed to start (or resume) one agent run.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub run: RunId,
    pub session: SessionId,
    /// The worktree the agent acts in. Also determines where its transcript is written.
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub permission_mode: PermissionMode,
    /// Tools the agent may use without prompting, on top of `permission_mode`.
    pub allow: ToolAllowlist,
    /// Resume an existing session rather than starting fresh.
    pub resume: Resume,
    /// Extra environment for the agent process. Credentials are **not** passed here —
    /// the agent uses the host node's own auth (ADR-0002). If something ever needs a
    /// token in this map, that is a design error, not a configuration one.
    pub env: Vec<(String, String)>,
    /// A file holding the MCP servers this run was granted (ADR-0011), or `None` when it was
    /// granted none — which is nearly every run.
    ///
    /// A **path rather than the JSON**, unlike the settings block beside it, and the difference
    /// is what the two contain: a resource's configuration can carry a credential for the
    /// service it reaches, and an argument is world-readable through `/proc/<pid>/cmdline` on a
    /// default Linux. The node writes it `0600` and removes it when the leg ends.
    ///
    /// Built by the node rather than here, because *which* servers a run may reach is a grant and
    /// what one looks like on a command line is this adapter's business. `None` and an empty set
    /// come to the same thing beside `--strict-mcp-config`; they are kept distinct so the command
    /// line says which happened.
    pub mcp: Option<PathBuf>,
    /// Where the agent should send a permission question, if this run may ask one (ADR-0017).
    ///
    /// `None` is the behaviour that has always existed: a tool call the run does not already
    /// permit is denied, headless, and recorded as a denial.
    pub ask: Option<AskHook>,
}

/// How a run asks a human for permission: a program the daemon nominates, and how long the
/// agent should wait for it (ADR-0017).
///
/// A hook rather than an MCP server, because a hook *is* the agent's own protocol for this and
/// because the program can be this node's own daemon binary — no discovery, no configuration,
/// and no way to point it at the wrong version.
///
/// The timeout is a backstop rather than the decision: the daemon answers on its own patience
/// (`Run::approval_patience`), and this is only what stops a blocked tool call outliving the
/// process that was going to answer it. Measured on `claude 2.1.237`: a hook killed at its
/// timeout leaves the tool **denied**, so the failure direction is the safe one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskHook {
    /// Program to run, absolute. Invoked with `args` and the agent's payload on stdin.
    pub command: PathBuf,
    pub args: Vec<String>,
    pub timeout: std::time::Duration,
    /// The tools worth asking a person about.
    ///
    /// A deliberately short list rather than everything: `PreToolUse` fires for *every* tool
    /// call, so a hook with no matcher would have us asking about reads the agent would have
    /// allowed on its own — the harness second-guessing the agent, which ADR-0004 forbids.
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// A new conversation.
    Fresh,
    /// Continue an existing session.
    ///
    /// `fork` mints a new session id on the agent's side. Use it when resuming after a
    /// migration where the previous holder might still be alive: two processes resuming
    /// the same session id would interleave writes into one transcript, and epoch fencing
    /// stops the *effects* but not that corruption.
    Continue { prompt: String, fork: bool },
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent binary `{binary}` not found or not executable")]
    NotInstalled { binary: String },
    #[error("failed to spawn agent: {0}")]
    Spawn(String),
    #[error("workspace {0} does not exist")]
    NoWorkspace(PathBuf),
    #[error("agent exited before reporting a result (status {status})")]
    NoOutcome { status: String },
    #[error(transparent)]
    Transcript(#[from] transcript::TranscriptError),
    /// The agent could not say which models it offers (ADR-0080).
    #[error("{0}")]
    Models(String),
}

/// What an adapter has to be able to do.
///
/// Deliberately does not assume "subprocess" — only spawn, stream, and resume — so an
/// agent with a good embeddable SDK could be adapted without reshaping this trait.
pub trait Agent {
    fn kind(&self) -> AgentKind;

    /// Version string as the agent itself reports it. Used for the migration eligibility
    /// check: a transcript may not be readable by an older agent (ADR-0003).
    fn version(&self) -> Option<String>;
}

/// Map Offload's permission model onto the agent's.
///
/// Note the headless caveat on [`PermissionMode::Ask`]: in `--print` mode there is nobody
/// to answer a prompt, so a run that actually hits a permission gate will record denials
/// rather than blocking for input. Surfacing those prompts to whichever device the user
/// is holding is roadmap question #1 and is not solved here — until it is, `Ask` runs are
/// only appropriate for work that stays inside its granted permissions.
#[must_use]
pub fn permission_mode_flag(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "manual",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Full => "bypassPermissions",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_unique_and_uuid_shaped() {
        let a = SessionId::generate();
        let b = SessionId::generate();
        assert_ne!(a, b);
        // The agent validates that --session-id is a UUID.
        assert_eq!(a.as_str().len(), 36);
        assert_eq!(a.as_str().matches('-').count(), 4);
    }

    #[test]
    fn permission_modes_map_to_real_cli_values() {
        // These strings are validated by the agent's own flag parser — a typo here is a
        // spawn failure at run time, not a compile error.
        assert_eq!(permission_mode_flag(PermissionMode::Ask), "manual");
        assert_eq!(
            permission_mode_flag(PermissionMode::AcceptEdits),
            "acceptEdits"
        );
        assert_eq!(
            permission_mode_flag(PermissionMode::Full),
            "bypassPermissions"
        );
    }
}
