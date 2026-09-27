//! The Claude Code adapter.
//!
//! Everything specific to `claude` lives here: its flags, its event stream, where it keeps
//! transcripts. The flags below were read off `claude --help` (2.1.220) and exercised
//! against a real run, not recalled.

use crate::event::{AgentEvent, TurnTracker};
use crate::{permission_mode_flag, Agent, AgentError, AskHook, Resume, SpawnRequest};
use offload_core::AgentKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// Drives the `claude` CLI.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    binary: PathBuf,
    /// Where this agent keeps its state — transcripts included. Resolved once, here, so the
    /// environment is read in exactly one place and everything downstream is handed an answer
    /// rather than a question (see `transcript::config_dir`).
    config: PathBuf,
    version: Option<String>,
}

impl ClaudeCode {
    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        ClaudeCode {
            binary: PathBuf::from("claude"),
            config: crate::transcript::config_dir(&home),
            version: None,
        }
    }

    /// Point the adapter at a config directory directly, instead of deriving one.
    ///
    /// For tests and for anything that already knows the answer. [`ClaudeCode::new`] reads the
    /// environment, which is right in production and wrong in a test: a temporary directory that
    /// silently became `$CLAUDE_CONFIG_DIR` would have the suite writing into somebody's real
    /// agent state.
    #[must_use]
    pub fn with_config_dir(config: PathBuf) -> Self {
        ClaudeCode {
            binary: PathBuf::from("claude"),
            config,
            version: None,
        }
    }

    /// Where this agent keeps its state, which is not always under `$HOME`.
    #[must_use]
    pub fn config_dir(&self) -> &std::path::Path {
        &self.config
    }

    #[must_use]
    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = binary;
        self
    }

    #[must_use]
    pub fn with_version(mut self, version: String) -> Self {
        self.version = Some(version);
        self
    }

    /// The models this agent's account can use, as the agent itself lists them (ADR-0080).
    ///
    /// Asked with the `initialize` request of the streaming-JSON control protocol — the one the
    /// Agent SDK's `supportedModels()` sends — and nothing else: no prompt, so no model is
    /// called. Run as a spawn would be, with [`agent_env`]'s environment, because the answer is
    /// per account and per version. Measured on `claude 2.1.283`: about 2 s, an exit of its own
    /// once stdin closes, and the same answer with the network cut. Blocking: callers on an
    /// async task use `spawn_blocking`.
    ///
    /// # Errors
    ///
    /// [`AgentError::Models`] when the agent does not answer within [`MODELS_TIMEOUT`], answers
    /// with an error, or answers with something that is not a model list.
    pub fn models(&self) -> Result<Vec<offload_core::Model>, AgentError> {
        self.models_within(MODELS_TIMEOUT)
    }

    fn models_within(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Vec<offload_core::Model>, AgentError> {
        use std::io::{BufRead, Write};
        let mut child = std::process::Command::new(&self.binary)
            .args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
            ])
            .envs(agent_env(&self.config, &[]))
            // Somewhere that is no repository: the agent keeps per-directory state, and the
            // daemon's own working directory is wherever it happened to be started.
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AgentError::Models(format!("could not start the agent: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let wrote = writeln!(stdin, "{MODELS_REQUEST}");
            // Dropped here: a closed stdin is what tells the agent it has nothing else to do.
            drop(stdin);
            if let Err(e) = wrote {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentError::Models(format!("could not ask the agent: {e}")));
            }
        }
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AgentError::Models("the agent has no output".into()));
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(answer) = models_answer(&line) {
                    let _ = tx.send(answer);
                    return;
                }
            }
        });
        let answer = rx.recv_timeout(timeout);
        // Whatever it said, it has nothing more to say to us. Killing a process that already
        // exited is harmless, and waiting reaps it either way.
        let _ = child.kill();
        let _ = child.wait();
        match answer {
            Ok(answer) => answer,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(AgentError::Models(format!(
                "the agent did not list its models within {}s",
                timeout.as_secs()
            ))),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(AgentError::Models(
                "the agent exited without listing its models".into(),
            )),
        }
    }

    /// Start the agent and stream its events.
    ///
    /// The returned [`RunHandle`] owns the child process; dropping it kills the agent.
    pub async fn spawn(&self, req: &SpawnRequest) -> Result<RunHandle, AgentError> {
        if !req.cwd.is_dir() {
            return Err(AgentError::NoWorkspace(req.cwd.clone()));
        }

        let argv = build_argv(req);
        tracing::info!(
            run_id = %req.run,
            session = %req.session,
            cwd = %req.cwd.display(),
            "spawning claude code"
        );

        let mut cmd = tokio::process::Command::new(&self.binary);
        cmd.args(&argv)
            .current_dir(&req.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        cmd.envs(child_env(&self.config, req));

        // Put the agent in its own process group so cancelling takes its whole subprocess
        // tree with it. An agent that has shelled out to a build leaves orphans otherwise,
        // and those orphans keep writing into a worktree we are about to migrate.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::NotInstalled {
                    binary: self.binary.display().to_string(),
                }
            } else {
                AgentError::Spawn(e.to_string())
            }
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentError::Spawn("no stdout pipe".into()))?;
        let stderr = child.stderr.take();

        // Bounded so a consumer that stops reading applies backpressure to the reader task
        // rather than growing an unbounded queue. A long agent run emits a lot.
        let (tx, rx) = mpsc::channel(256);

        let reader_tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut tracker = TurnTracker::new();
            while let Ok(Some(line)) = lines.next_line().await {
                for event in tracker.observe(&line) {
                    if reader_tx.send(event).await.is_err() {
                        return; // consumer went away
                    }
                }
            }
        });

        // Agent stderr is diagnostics, not run output — log it, don't route it into the
        // event stream where it would be mistaken for agent text.
        if let Some(stderr) = stderr {
            let run = req.run;
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(run_id = %run, "agent stderr: {line}");
                }
            });
        }

        Ok(RunHandle {
            session: req.session.0.clone(),
            cwd: req.cwd.clone(),
            config: self.config.clone(),
            events: rx,
            child: Some(child),
        })
    }

    /// Ask the installed binary what version it is.
    pub async fn detect_version(&self) -> Option<String> {
        let out = tokio::process::Command::new(&self.binary)
            .arg("--version")
            .output()
            .await
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.split_whitespace()
            .find(|tok| tok.contains('.') && tok.starts_with(|c: char| c.is_ascii_digit()))
            .map(str::to_string)
    }
}

impl Agent for ClaudeCode {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn version(&self) -> Option<String> {
        self.version.clone()
    }
}

/// A live agent process.
#[derive(Debug)]
pub struct RunHandle {
    session: String,
    cwd: PathBuf,
    config: PathBuf,
    events: mpsc::Receiver<AgentEvent>,
    child: Option<tokio::process::Child>,
}

impl RunHandle {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session
    }

    /// Next event, or `None` once the agent's output stream has closed.
    pub async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    /// The agent's process group id, which is also its pid — it is spawned with
    /// `process_group(0)`.
    ///
    /// Exposed because it is the only durable handle on a running agent, and this one lives in
    /// memory: a daemon that is *killed* rather than shut down takes the handle with it and
    /// leaves the agent running. Whoever wants to be able to stop it later has to write this
    /// number down somewhere that outlives the process holding it.
    #[must_use]
    pub fn process_group(&self) -> Option<u32> {
        self.child.as_ref().and_then(tokio::process::Child::id)
    }

    /// Where this run's transcript currently lives.
    pub fn transcript_path(&self) -> Result<PathBuf, AgentError> {
        Ok(crate::transcript::find(
            &self.config,
            &self.cwd,
            &self.session,
        )?)
    }

    /// Stop the agent and its whole process group.
    ///
    /// Sends SIGTERM to the group, waits briefly, then SIGKILLs. Signalling only the
    /// direct child would leave whatever it shelled out to still running.
    pub async fn cancel(&mut self, grace: std::time::Duration) -> Result<(), AgentError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        let Some(pid) = child.id() else {
            return Ok(()); // already reaped
        };

        #[cfg(unix)]
        signal_group(pid, libc_sigterm());

        // Give the agent a chance to exit cleanly before escalating.
        if tokio::time::timeout(grace, child.wait()).await.is_ok() {
            self.child = None;
            return Ok(());
        }

        tracing::warn!(pid, "agent did not exit on SIGTERM; killing process group");
        #[cfg(unix)]
        signal_group(pid, libc_sigkill());
        let _ = child.kill().await;
        self.child = None;
        Ok(())
    }

    /// Wait for the agent to exit on its own.
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus, AgentError> {
        let child = self
            .child
            .as_mut()
            .ok_or_else(|| AgentError::Spawn("process already reaped".into()))?;
        child
            .wait()
            .await
            .map_err(|e| AgentError::Spawn(e.to_string()))
    }
}

#[cfg(unix)]
const fn libc_sigterm() -> i32 {
    15
}

#[cfg(unix)]
const fn libc_sigkill() -> i32 {
    9
}

/// Ask an agent's whole process group to stop.
///
/// Public because the group may belong to an agent this process never spawned — one a previous
/// incarnation of the daemon left behind, whose `Child` died with it. There is no handle to wait
/// on in that case, so the caller decides how long to wait and then calls [`kill_process_group`].
#[cfg(unix)]
pub fn terminate_process_group(pid: u32) {
    signal_group(pid, libc_sigterm());
}

/// Kill an agent's whole process group outright. See [`terminate_process_group`].
#[cfg(unix)]
pub fn kill_process_group(pid: u32) {
    signal_group(pid, libc_sigkill());
}

/// Signal an entire process group. The child was spawned with `process_group(0)`, so its
/// pid is also its process-group id.
#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) {
    // SAFETY-adjacent note: `unsafe` is forbidden workspace-wide, so this shells out to
    // `kill` rather than calling libc. Slower, but this runs once per cancellation and
    // keeps the crate free of unsafe code.
    let target = format!("-{pid}");
    let _ = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(&target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// How long [`ClaudeCode::models`] waits for the agent's answer. Measured at about 2 s; the rest
/// is room for a slow disk or a first start, not for a network the answer does not need.
pub const MODELS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The one request [`ClaudeCode::models`] sends.
const MODELS_REQUEST: &str = r#"{"type":"control_request","request_id":"offload-models","request":{"subtype":"initialize"}}"#;

/// The model list out of one line of the agent's output, if this line is the answer to
/// [`MODELS_REQUEST`]. `None` for every other line; `Some(Err)` for an answer that is not a list.
fn models_answer(line: &str) -> Option<Result<Vec<offload_core::Model>, AgentError>> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "control_response" {
        return None;
    }
    let response = value.get("response")?;
    if response.get("request_id").and_then(|v| v.as_str()) != Some("offload-models") {
        return None;
    }
    if response.get("subtype").and_then(|v| v.as_str()) != Some("success") {
        let why = response
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("no reason given");
        return Some(Err(AgentError::Models(format!(
            "the agent refused to list its models: {why}"
        ))));
    }
    let Some(listed) = response
        .get("response")
        .and_then(|r| r.get("models"))
        .and_then(|m| m.as_array())
    else {
        return Some(Err(AgentError::Models(
            "the agent answered without a model list".into(),
        )));
    };
    let text =
        |m: &serde_json::Value, key: &str| m.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Some(Ok(listed
        .iter()
        .filter_map(|m| {
            // A model with no value cannot be passed to `--model`, so it is not offered.
            let value = text(m, "value").filter(|v| !v.is_empty())?;
            Some(offload_core::Model {
                name: text(m, "displayName").unwrap_or_else(|| value.clone()),
                about: text(m, "description").unwrap_or_default(),
                resolves_to: text(m, "resolvedModel").unwrap_or_else(|| value.clone()),
                value,
            })
        })
        .collect()))
}

/// The agent's environment beyond what it inherits, in the order it is applied.
///
/// Pure and split out for the same reason [`build_argv`] is: the order is the rule, and a
/// test can only pin an order it can see.
#[must_use]
pub fn child_env(config: &Path, req: &SpawnRequest) -> Vec<(String, String)> {
    agent_env(config, &req.env)
}

/// [`child_env`] for anything that starts the agent, a run or not: [`ClaudeCode::models`] asks
/// the account's model list and must ask *as* the account a run would use.
#[must_use]
pub fn agent_env(config: &Path, extra: &[(String, String)]) -> Vec<(String, String)> {
    // The child is told where its state is rather than left to inherit it (ADR-0028). It
    // used to inherit, on the stated grounds that "the agent is spawned as a child and
    // inherits it, so the two cannot disagree" — true only while nothing else could set the
    // path. Once an owner can nominate one in `node.toml`, inheritance is exactly the
    // disagreement: the capture would look where the config says and the agent would write
    // where the shell that started the daemon says, which is a run checkpointing nothing all
    // night with only its own log to say so.
    let mut env = vec![(
        "CLAUDE_CONFIG_DIR".to_string(),
        config.display().to_string(),
    )];

    // After the config directory, so a caller that means to override it still can — the
    // resource proxy's environment is the owner's word about one server, and this is the
    // adapter's word about its own state.
    env.extend(extra.iter().cloned());

    // Auto-memory is **off**, unconditionally, and last so nothing above can turn it back on
    // (ADR-0065). The agent keys its memory by *repository*, not by worktree, and keeps it on
    // the machine: measured on `claude 2.1.281`, a run in one worktree of a mirror saved a
    // codeword and a fresh run in a *second* worktree of the same mirror answered it in one
    // turn with no tool call. So with it on, what a run knows depends on which node won the
    // bid and on every earlier run of that repository there — the argument that makes
    // `--strict-mcp-config` unconditional, about memory rather than reach. With it off, the
    // same request saved its note as a file in the worktree, which the checkpoint carries.
    env.push((
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_string(),
        "1".to_string(),
    ));
    env
}

/// Build the agent's command line.
///
/// Split out and pure so the flag combinations are unit-testable — getting these wrong is
/// a runtime spawn failure, and the interesting cases (resume, fork, model selection) are
/// exactly the ones that only occur during a migration.
#[must_use]
pub fn build_argv(req: &SpawnRequest) -> Vec<String> {
    // `--print` is non-interactive mode; `stream-json` additionally requires `--verbose`.
    //
    // `--strict-mcp-config` is **unconditional**, and it is the flag that decides what a run can
    // reach beyond its own worktree (ADR-0011). Without it the agent loads whatever MCP servers
    // the *host user* has configured for themselves, plus any `.mcp.json` the repo happens to
    // ship — so what a run could touch would depend on which machine won the bid, and a repo
    // could grant itself a service by committing a file. Measured on `claude 2.1.238`, on a
    // developer laptop and not a contrived one: without the flag a run inherited four of the
    // owner's connected servers, mail and calendar among them.
    //
    // There is no configuration under which that is the right default, which is why this is not
    // configurable. A run reaches exactly what the fleet granted it, the same rule the tool
    // allowlist already enforces, and a grant is a decision somebody made rather than an accident
    // of placement. With the flag and no `--mcp-config`, the run's MCP set is empty — measured,
    // not assumed.
    let mut argv: Vec<String> = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--strict-mcp-config".into(),
    ];

    // The prompt is positional and goes at the very end, behind `--`; only the flags that
    // identify the session are decided here.
    let prompt = match &req.resume {
        Resume::Fresh => {
            // Assigning the session id up front is what makes the transcript findable
            // later without parsing it back out of the stream.
            argv.push("--session-id".into());
            argv.push(req.session.0.clone());
            req.prompt.clone()
        }
        Resume::Continue { prompt, fork } => {
            argv.push("--resume".into());
            argv.push(req.session.0.clone());
            if *fork {
                argv.push("--fork-session".into());
            }
            prompt.clone()
        }
    };

    if let Some(model) = &req.model {
        argv.push("--model".into());
        argv.push(model.clone());
    }

    argv.push("--permission-mode".into());
    argv.push(permission_mode_flag(req.permission_mode).into());

    // Exactly what the fleet granted, beside the flag that makes "exactly" true. A path and not
    // JSON, unlike the settings block below: a resource's configuration can carry a credential
    // for the service it reaches, and an argument is readable by any local user through
    // `/proc/<pid>/cmdline`. Measured rather than assumed — `/proc` is mounted without `hidepid`
    // by default — and the file the node writes is `0600`.
    if let Some(mcp) = &req.mcp {
        argv.push("--mcp-config".into());
        argv.push(mcp.display().to_string());
    }

    // A `PreToolUse` hook is how a run asks a person for permission (ADR-0017). Passed as JSON
    // rather than a file so there is nothing to clean up and nothing to leave behind pointing at
    // a socket that has moved.
    if let Some(ask) = &req.ask {
        argv.push("--settings".into());
        argv.push(ask_settings(ask));
    }

    // One argv entry per pattern: patterns contain spaces (`Bash(cargo test:*)`), so a single
    // space-joined argument would be ambiguous. `--allowedTools` is variadic and swallows
    // everything after it, which is why it is the last *flag* — and why the `--` below is what
    // makes anything able to follow it at all.
    if !req.allow.is_empty() {
        argv.push("--allowedTools".into());
        argv.extend(req.allow.to_args());
    }

    // The prompt, last and behind the option terminator.
    //
    // Without `--`, a prompt that begins with a dash is read as a flag: measured on `claude
    // 2.1.238`, "--version is what I want documented" exits immediately with `error: unknown
    // option`. That is an ordinary thing to ask an agent about a CLI — and it is a run that
    // fails at spawn for a reason nothing in the fleet can explain, retried by an unattended
    // run's own recovery to exactly the same end.
    //
    // Both halves of this are measured against 2.1.238: `--` terminates the variadic
    // `--allowedTools` rather than being taken as another pattern, and the grants before it
    // still apply — a run allowed `Bash(cargo build:*)` and given a dash-leading prompt after
    // `--` ran its command under `--permission-mode default` with no denials.
    argv.push("--".into());
    argv.push(prompt);

    argv
}

/// One line describing what a tool call is about to do, from the agent's own `tool_input`.
///
/// Two jobs, and they want the same string: it is what a person reads on a phone
/// ("cargo build --release"), and it is what an existing grant is matched against
/// (`ToolAllowlist::covers`). Keeping them the same string is deliberate — a question that
/// shows one thing and matches another is a question nobody can answer safely.
///
/// Here rather than in `offload-node` because the *shape* of a tool's input is the agent's
/// business: `Bash` calls it `command`, `WebFetch` calls it `url` (ADR-0004).
#[must_use]
pub fn describe_tool_call(tool: &str, input: &serde_json::Value) -> String {
    let field = match tool {
        "Bash" | "BashOutput" => "command",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "Read" => "file_path",
        _ => "",
    };
    if let Some(text) = input.get(field).and_then(serde_json::Value::as_str) {
        return one_line(text);
    }
    // Unknown tool, or an input shaped differently from what this build knows: the whole input,
    // compactly. Better a dense line than an empty question.
    one_line(&input.to_string())
}

/// Collapse to a single line and bound the length.
///
/// A tool input can be an entire file. This ends up in a log line, a notification title and a
/// terminal column, none of which survive a newline.
fn one_line(text: &str) -> String {
    const MAX: usize = 160;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX {
        return flat;
    }
    let kept: String = flat.chars().take(MAX).collect();
    format!("{kept}…")
}

/// The settings block that declares the permission hook.
///
/// Built with `serde_json` rather than a format string, for the reason a sink has no template
/// language: a hand-written JSON string containing a path is a quoting bug with an audience.
///
/// The `command` field *is* run through a shell by the agent, so the program and its arguments
/// are quoted here. One matcher per tool rather than one hook with no matcher: `PreToolUse` fires
/// for every tool call, and a hook that asks about a `Read` is asking about something the agent
/// would have allowed by itself.
#[must_use]
pub fn ask_settings(ask: &AskHook) -> String {
    let mut command = shell_quote(&ask.command.display().to_string());
    for arg in &ask.args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    let entries: Vec<serde_json::Value> = ask
        .tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "matcher": tool,
                "hooks": [{
                    "type": "command",
                    "command": command,
                    "timeout": ask.timeout.as_secs().max(1),
                }],
            })
        })
        .collect();
    serde_json::json!({ "hooks": { "PreToolUse": entries } }).to_string()
}

/// Single-quote a string for a POSIX shell.
///
/// Everything is literal inside single quotes, and the one character that cannot appear there is
/// closed, escaped and reopened. Short, total, and here because the alternative — assuming paths
/// have no spaces in them — is the assumption that holds until somebody installs into a directory
/// with a space in its name.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The default nudge used when resuming a migrated run.
///
/// Resume is not perfectly transparent: `--print` needs *a* prompt, and re-sending the
/// original would restart the task rather than continue it. This is the honest compromise
/// — the conversation and worktree carry the real state, and this only tells the agent to
/// pick up where it stopped.
///
/// **It has to say "finish", and the first version did not.** Under `--print` a run ends at the
/// first assistant turn that makes no tool call, so a nudge that reads as "take stock and report"
/// ends the run — and the fleet records `Completed`, which is terminal, so nothing ever picks it
/// up again. Watched happening on a real migration: an agent thirty-one steps into a task resumed
/// on the new machine, did one more step, wrote a sentence saying which machine it was on, and
/// the run was recorded finished at six steps of thirty-one with nothing anywhere saying the rest
/// had been abandoned. It is the *silence* that makes this worth a change of words rather than a
/// shrug — a failed run is retried and an abandoned one looks like a success. So the nudge names
/// the original task and says to carry it through, and says in as many words that this is not a
/// request for a progress report.
pub const CONTINUE_PROMPT: &str =
    "Continue the task you were already given and carry it through to completion. This is \
     the same run resuming, not a request for a progress report: do not stop to summarise \
     what you have done so far — stop only when the whole task is finished. Your workspace \
     and conversation have been restored on a different machine, so check the current state \
     of the files before assuming any in-progress edit completed.";

#[must_use]
pub fn resume_after_migration() -> Resume {
    Resume::Continue {
        prompt: CONTINUE_PROMPT.to_string(),
        // Fork so a still-alive previous holder cannot interleave writes into the same
        // transcript. Epoch fencing stops its side effects; it does not stop it writing
        // to a file we are now also writing to.
        fork: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionId;
    use offload_core::{PermissionMode, RunId};

    fn request() -> SpawnRequest {
        SpawnRequest {
            run: RunId::from_bytes([1; 16]),
            session: SessionId("11111111-2222-3333-4444-555555555555".into()),
            cwd: PathBuf::from("/work/repo"),
            prompt: "add tests for the parser".into(),
            model: None,
            permission_mode: PermissionMode::Ask,
            allow: offload_core::ToolAllowlist::default(),
            resume: Resume::Fresh,
            env: Vec::new(),
            mcp: None,
            ask: None,
        }
    }

    fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        let i = argv.iter().position(|a| a == flag)?;
        argv.get(i + 1).map(String::as_str)
    }

    /// The value the child ends up with for `key`: `Command::envs` applies in order, so the
    /// last entry wins.
    fn effective<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn auto_memory_is_off_and_nothing_in_the_request_turns_it_back_on() {
        let mut req = request();
        req.env = vec![
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "0".into()),
            ("CLAUDE_CONFIG_DIR".into(), "/elsewhere".into()),
        ];
        let env = child_env(Path::new("/state/claude"), &req);
        assert_eq!(
            effective(&env, "CLAUDE_CODE_DISABLE_AUTO_MEMORY"),
            Some("1")
        );
        // …while the config directory stays overridable, which is the documented order.
        assert_eq!(effective(&env, "CLAUDE_CONFIG_DIR"), Some("/elsewhere"));
        assert_eq!(
            effective(
                &child_env(Path::new("/state/claude"), &request()),
                "CLAUDE_CONFIG_DIR"
            ),
            Some("/state/claude")
        );
    }

    #[test]
    fn a_fresh_run_assigns_its_own_session_id() {
        // The whole transcript-location strategy rests on this: we choose the id, so we
        // never have to scrape it back out of the event stream.
        let argv = build_argv(&request());
        assert_eq!(
            flag_value(&argv, "--session-id"),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert!(!argv.iter().any(|a| a == "--resume"));
        assert!(argv.contains(&"add tests for the parser".to_string()));
    }

    #[test]
    fn stream_json_always_comes_with_verbose() {
        // The agent rejects stream-json without it; without this pairing every run fails
        // to start.
        let argv = build_argv(&request());
        assert_eq!(flag_value(&argv, "--output-format"), Some("stream-json"));
        assert!(argv.contains(&"--verbose".to_string()));
        assert!(argv.contains(&"--print".to_string()));
    }

    #[test]
    fn a_granted_resource_is_named_by_path_and_never_spelled_out() {
        // The config can carry a credential for the service it reaches, and an argument is
        // readable by any local user through `/proc/<pid>/cmdline`.
        let mut req = request();
        req.mcp = Some(PathBuf::from("/state/mcp/run.json"));
        let argv = build_argv(&req);
        assert_eq!(
            flag_value(&argv, "--mcp-config"),
            Some("/state/mcp/run.json")
        );
        assert!(argv.contains(&"--strict-mcp-config".to_string()));

        // And a run granted nothing says nothing, rather than passing an empty set: beside the
        // strict flag they mean the same to the agent and different things to whoever reads the
        // command line afterwards.
        assert!(!build_argv(&request()).iter().any(|a| a == "--mcp-config"));
    }

    #[test]
    fn every_run_is_spawned_with_nothing_ambient_to_reach() {
        // ADR-0011's line, and the one flag that enforces it. Without it the agent loads the
        // host user's own MCP servers and any `.mcp.json` the repo ships — so what a run could
        // touch would depend on which machine won the bid, and a repo could grant itself a
        // service by committing a file.
        //
        // Asserted on *every* shape of request rather than on one, because the failure is silent
        // and the flag is easy to lose behind a branch: a resumed run that dropped it would be
        // more dangerous than a fresh one, having already been trusted once.
        let resumed = SpawnRequest {
            resume: Resume::Continue {
                prompt: "keep going".into(),
                fork: true,
            },
            ..request()
        };
        let permissive = SpawnRequest {
            permission_mode: PermissionMode::Full,
            ..request()
        };
        for req in [request(), resumed, permissive] {
            assert!(
                build_argv(&req).contains(&"--strict-mcp-config".to_string()),
                "a run must reach nothing the fleet did not grant it"
            );
        }
    }

    #[test]
    fn resuming_targets_the_session_and_does_not_reassign_it() {
        // `--session-id` and `--resume` are mutually exclusive: passing both makes the
        // agent reject the invocation.
        let mut req = request();
        req.resume = Resume::Continue {
            prompt: "keep going".into(),
            fork: false,
        };
        let argv = build_argv(&req);
        assert_eq!(
            flag_value(&argv, "--resume"),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert!(!argv.iter().any(|a| a == "--session-id"));
        assert!(!argv.iter().any(|a| a == "--fork-session"));
    }

    #[test]
    fn the_continue_prompt_tells_a_migrated_agent_to_finish_rather_than_report() {
        // Under `--print` a run ends at the first assistant turn with no tool call, so what this
        // sentence asks for is what decides whether a migrated run carries on or stops. The
        // version that only said "continue from where you left off" was read once, on a real
        // migration, as "take a step and tell me where you are": the run was recorded `Completed`
        // at six steps of thirty-one, which is terminal, so nothing retried it and nothing said
        // the rest had been abandoned.
        assert!(
            CONTINUE_PROMPT.contains("completion"),
            "it has to ask for the whole task: {CONTINUE_PROMPT}"
        );
        assert!(
            CONTINUE_PROMPT.contains("not a request for a progress report"),
            "and rule out the reading that ended the run: {CONTINUE_PROMPT}"
        );
        // And it is one paragraph rather than a literal somebody broke. A `\` continuation drops
        // the newline and the indentation, but a tool that rewrites this file through a language
        // whose own strings keep the indentation bakes a run of spaces into the middle of the
        // sentence — invisible in review, wrong only when a machine is being spoken to.
        assert!(
            !CONTINUE_PROMPT.contains("  "),
            "the literal came out mangled: {CONTINUE_PROMPT:?}"
        );
    }

    #[test]
    fn migration_resume_forks_the_session() {
        // Forking is what stops a still-alive previous holder from writing into the same
        // transcript we just restored.
        let mut req = request();
        req.resume = resume_after_migration();
        let argv = build_argv(&req);
        assert!(argv.contains(&"--fork-session".to_string()));
        assert!(argv.contains(&CONTINUE_PROMPT.to_string()));
    }

    #[test]
    fn model_is_only_passed_when_chosen() {
        let argv = build_argv(&request());
        assert!(!argv.iter().any(|a| a == "--model"));

        let mut req = request();
        req.model = Some("claude-opus-5".into());
        assert_eq!(
            flag_value(&build_argv(&req), "--model"),
            Some("claude-opus-5")
        );
    }

    #[test]
    fn an_allowlist_is_the_last_flag_one_entry_per_pattern() {
        // `--allowedTools` is variadic: anything after it gets swallowed, except the option
        // terminator. And patterns contain spaces, so they cannot be joined into one
        // argument.
        let mut req = request();
        req.allow =
            offload_core::ToolAllowlist::parse(["Bash(cargo test:*)", "Edit"]).expect("valid");
        let argv = build_argv(&req);

        let at = argv
            .iter()
            .position(|a| a == "--allowedTools")
            .expect("flag present");
        assert_eq!(
            &argv[at + 1..],
            &[
                "Bash(cargo test:*)".to_string(),
                "Edit".into(),
                "--".into(),
                "add tests for the parser".into()
            ],
            "the patterns, then the terminator, then the prompt"
        );
    }

    #[test]
    fn a_prompt_that_begins_with_a_dash_is_still_a_prompt() {
        // Measured on `claude 2.1.238`: without `--`, this exits at once with `error: unknown
        // option '--version is what I want documented'`. Asking an agent about a flag is an
        // ordinary thing to ask, and the failure is a run that never starts for a reason
        // nothing in the fleet can explain.
        let mut req = request();
        req.prompt = "--version is what I want documented".into();
        req.allow = offload_core::ToolAllowlist::parse(["Bash(cargo test:*)"]).expect("valid");
        let argv = build_argv(&req);

        assert_eq!(
            argv.last().map(String::as_str),
            Some("--version is what I want documented")
        );
        assert_eq!(
            argv.iter().filter(|a| *a == "--").count(),
            1,
            "exactly one terminator, and the prompt is what follows it"
        );
        assert_eq!(
            argv[argv.len() - 2],
            "--",
            "so the agent's own parser stops before it: {argv:?}"
        );
    }

    #[test]
    fn no_allowlist_means_no_flag() {
        // A bare `--allowedTools` with nothing after it would change behaviour rather
        // than leave it alone.
        assert!(!build_argv(&request()).iter().any(|a| a == "--allowedTools"));
    }

    #[test]
    fn the_ask_hook_reaches_the_command_line_as_a_settings_block() {
        let mut req = request();
        assert!(
            !build_argv(&req).contains(&"--settings".to_string()),
            "a run that may not ask carries no hook at all"
        );

        req.ask = Some(crate::AskHook {
            command: PathBuf::from("/opt/my offload/offloadd"),
            args: vec!["ask-hook".into()],
            timeout: std::time::Duration::from_secs(310),
            tools: vec!["Bash".into(), "WebFetch".into()],
        });
        let argv = build_argv(&req);
        let settings = flag_value(&argv, "--settings").expect("a settings block");
        let json: serde_json::Value = serde_json::from_str(settings).expect("valid JSON");

        let hooks = json["hooks"]["PreToolUse"]
            .as_array()
            .expect("one entry per tool");
        assert_eq!(hooks.len(), 2, "one matcher per tool, not one hook for all");
        assert_eq!(hooks[0]["matcher"], "Bash");
        assert_eq!(hooks[1]["matcher"], "WebFetch");

        let inner = &hooks[0]["hooks"][0];
        assert_eq!(inner["type"], "command");
        assert_eq!(inner["timeout"], 310);
        // The agent runs this through a shell, and the path has a space in it. Quoted, or the
        // hook is "/opt/my" with an argument nobody meant.
        assert_eq!(
            inner["command"], "'/opt/my offload/offloadd' 'ask-hook'",
            "the program and its arguments are shell-quoted"
        );
    }

    #[test]
    fn a_path_with_a_quote_in_it_still_survives_the_shell() {
        // Total rather than nearly total: the one character that cannot appear inside single
        // quotes is the one somebody will eventually have in a directory name.
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
    }

    #[test]
    fn permission_mode_reaches_the_command_line() {
        let mut req = request();
        req.permission_mode = PermissionMode::AcceptEdits;
        assert_eq!(
            flag_value(&build_argv(&req), "--permission-mode"),
            Some("acceptEdits")
        );
    }

    #[tokio::test]
    async fn spawning_into_a_missing_workspace_fails_before_launching_anything() {
        let mut req = request();
        req.cwd = PathBuf::from("/definitely/not/a/real/path");
        let agent = ClaudeCode::new(PathBuf::from("/home/nobody"));
        assert!(matches!(
            agent.spawn(&req).await,
            Err(AgentError::NoWorkspace(_))
        ));
    }

    #[tokio::test]
    async fn the_agent_is_told_where_its_state_is_rather_than_left_to_inherit_it() {
        // ADR-0028. The adapter resolves the config directory — from the owner's `config_dir`
        // where there is one — and the *capture* looks there for the transcript. So the child has
        // to be told: inheriting means the agent writes wherever the shell that started the
        // daemon says, and the failure is a run checkpointing nothing all night with only its own
        // log to say so. Driven through a real spawn, because the bug is in the environment of a
        // process and nothing short of one has an environment.
        let dir = std::env::temp_dir().join(format!("offload-cfgdir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let binary = dir.join("echo-config-dir");
        // A raw string with no continuation: a `\`-newline in a literal drops the newline and
        // bakes the indentation into the value, which is a whole class of mangled text in this
        // repository and would silently corrupt the script.
        let script = format!(
            "#!/bin/sh\nprintf '{}' \"$CLAUDE_CONFIG_DIR\"\n",
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"%s"}
"#
        );
        std::fs::write(&binary, script).expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let nominated = dir.join("nominated-state");
        let agent = ClaudeCode::with_config_dir(nominated.clone()).with_binary(binary);
        let mut req = request();
        req.cwd = dir.clone();
        let mut handle = agent.spawn(&req).await.expect("spawn");

        let mut said = None;
        while let Some(event) = handle.next_event().await {
            if let AgentEvent::Finished(outcome) = event {
                said = outcome.result;
            }
        }
        assert_eq!(
            said.as_deref(),
            Some(nominated.to_str().expect("utf-8")),
            "the child must see the directory this adapter resolved, not the one this process started with"
        );
    }

    #[tokio::test]
    async fn a_missing_binary_is_reported_as_not_installed() {
        let mut req = request();
        req.cwd = std::env::temp_dir();
        let agent = ClaudeCode::new(PathBuf::from("/home/nobody"))
            .with_binary(PathBuf::from("offload-no-such-agent-binary"));
        assert!(matches!(
            agent.spawn(&req).await,
            Err(AgentError::NotInstalled { .. })
        ));
    }

    /// A fake agent for [`ClaudeCode::models`]: a script that reads the request line and then
    /// runs `body`. Its own directory, so two tests never exec the same freshly written file.
    fn models_agent(name: &str, body: &str) -> (std::path::PathBuf, ClaudeCode) {
        let dir =
            std::env::temp_dir().join(format!("offload-models-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let binary = dir.join("agent");
        std::fs::write(&binary, format!("#!/bin/sh\nread request\n{body}\n")).expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let agent = ClaudeCode::with_config_dir(dir.join("nominated-state")).with_binary(binary);
        (dir, agent)
    }

    #[test]
    fn the_models_are_the_agents_own_list_read_as_the_account_a_run_would_use() {
        // ADR-0080. The answer is per account, so the agent is asked with the environment a run
        // gets: the description echoes `$CLAUDE_CONFIG_DIR` to prove which account answered. A
        // line before the answer is ignored, as the agent's own chatter would be.
        let answer = r#"{"type":"control_response","response":{"subtype":"success","request_id":"offload-models","response":{"models":[{"value":"default","displayName":"Default (recommended)","description":"%s","resolvedModel":"claude-opus-5-5"},{"value":"haiku","displayName":"Haiku 4.5"},{"displayName":"no value, not offered"}]}}}"#;
        let (dir, agent) = models_agent(
            "answers",
            &format!("echo '{{\"type\":\"system\"}}'\nprintf '{answer}\\n' \"$CLAUDE_CONFIG_DIR\""),
        );
        let models = agent
            .models_within(std::time::Duration::from_secs(10))
            .expect("models");
        assert_eq!(
            models,
            vec![
                offload_core::Model {
                    value: "default".into(),
                    name: "Default (recommended)".into(),
                    about: dir.join("nominated-state").display().to_string(),
                    resolves_to: "claude-opus-5-5".into(),
                },
                offload_core::Model {
                    value: "haiku".into(),
                    name: "Haiku 4.5".into(),
                    about: String::new(),
                    resolves_to: "haiku".into(),
                },
            ]
        );
    }

    #[test]
    fn an_agent_that_refuses_to_list_its_models_says_why() {
        let (_dir, agent) = models_agent(
            "refuses",
            r#"echo '{"type":"control_response","response":{"subtype":"error","request_id":"offload-models","error":"not logged in"}}'"#,
        );
        let err = agent
            .models_within(std::time::Duration::from_secs(10))
            .expect_err("refused");
        assert_eq!(
            err.to_string(),
            "the agent refused to list its models: not logged in"
        );
    }

    #[test]
    fn an_agent_that_never_answers_is_given_up_on_not_waited_for() {
        // A hung agent must not hold the node's model reader for ever; the timeout is the bound.
        let (_dir, agent) = models_agent("hangs", "sleep 30");
        let started = std::time::Instant::now();
        let err = agent
            .models_within(std::time::Duration::from_secs(1))
            .expect_err("timed out");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            err.to_string(),
            "the agent did not list its models within 1s"
        );
    }

    #[test]
    fn an_agent_that_exits_without_answering_is_not_a_timeout() {
        let (_dir, agent) = models_agent("exits", "exit 3");
        let err = agent
            .models_within(std::time::Duration::from_secs(10))
            .expect_err("exited");
        assert_eq!(
            err.to_string(),
            "the agent exited without listing its models"
        );
    }
}
