//! `offloadd` — the node daemon.

use anyhow::{Context, Result};
use clap::Parser;
use offload_node::config::Config;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "offloadd", about = "Offload node daemon", version)]
struct Cli {
    /// TOML config file. Every setting has a working default, so this is optional.
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Override the state directory (mirrors, worktrees, run logs, identity).
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Log filter, e.g. "debug" or "offload_node=debug,info".
    #[arg(long, default_value = "info")]
    log: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Before anything else, and before the config is even read: this binary doubles as the
    // permission hook it hands its own agents (ADR-0017). Its own path is the one thing this
    // daemon can state exactly, which is why there is no configuration for where the hook lives
    // and no way to point it at a different version of the protocol.
    if std::env::args().nth(1).as_deref() == Some(offload_node::supervisor::ASK_HOOK_ARG) {
        return ask_hook().await;
    }
    // And, for the same reason, as the MCP server it hands an agent for a resource that is on
    // another machine (ADR-0011). `current_exe` again: nothing to discover, nothing to
    // configure, and no way to be pointed at a different version of the daemon than the one
    // that spawned the agent.
    if std::env::args().nth(1).as_deref() == Some("use-resource") {
        return use_resource().await;
    }

    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log)),
        )
        .init();

    // `~/.config/offload/node.toml` when none is named and it exists (ADR-0074).
    let path = offload_node::config::config_path(cli.config.as_deref());
    let mut config = Config::load(path.as_deref()).context("loading config")?;
    if let Some(dir) = cli.state_dir {
        config.state_dir = dir;
    }
    // Checkouts where a person reads them, `~/offload`, unless the config says where. Not on a
    // phone, where `~` is the app's private directory and nobody browses it.
    if !cfg!(target_os = "android") {
        config =
            config.with_readable_checkouts(std::env::var_os("HOME").map(PathBuf::from).as_deref());
    }
    match &path {
        Some(path) => tracing::info!(config = %path.display(), "config read"),
        None => tracing::info!("no config file; every setting is its default"),
    }
    offload_node::daemon::run(config, wait_for_signal()).await
}

/// Resolve on SIGTERM or SIGINT.
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGTERM");
                return;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGINT");
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM"),
            _ = int.recv() => tracing::info!("SIGINT"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Serve one permission question, as the agent's `PreToolUse` hook (ADR-0017).
///
/// Reads the agent's payload on stdin, asks the daemon that spawned the agent, and prints the
/// answer in the agent's own shape. Everything about it is deliberately small: it holds no state,
/// makes one request, and its whole failure mode is *print nothing*.
///
/// **Printing nothing is the safe answer, and it is what every unhappy path does here** — no
/// socket, no answer, a malformed payload, a daemon that has gone away. An empty hook response
/// leaves the agent's own rules in charge, which is exactly what happens on every run in the fleet
/// that never asked anybody: a gated command is refused, and one the agent would have allowed is
/// allowed. Measured on `claude 2.1.237` rather than assumed, including the case where the hook is
/// killed at its timeout.
async fn ask_hook() -> Result<()> {
    use tokio::io::AsyncReadExt;

    let mut payload = String::new();
    tokio::io::stdin().read_to_string(&mut payload).await.ok();

    let Some(question) = hook_question(&payload) else {
        return Ok(());
    };
    let Ok(socket) = std::env::var(offload_node::supervisor::ASK_SOCKET_ENV) else {
        return Ok(());
    };
    let Ok(run) = std::env::var(offload_node::supervisor::ASK_RUN_ENV) else {
        return Ok(());
    };
    let epoch = std::env::var(offload_node::supervisor::ASK_EPOCH_ENV)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or_default();

    let request = offload_node::api::Request::Ask {
        run,
        epoch,
        tool_use_id: question.tool_use_id,
        tool: question.tool,
        detail: question.detail,
    };
    // No timeout of its own: the agent kills this process at the hook timeout it was given, and
    // the daemon decides first (`ASK_HOOK_MARGIN`). Two clocks for one wait is how a question ends
    // up answered twice.
    let Some(answer) = ask_daemon(std::path::Path::new(&socket), &request).await else {
        return Ok(());
    };
    if let Some(rendered) = hook_response(&answer) {
        println!("{rendered}");
    }
    Ok(())
}

/// What the agent told us it wants to do.
struct Question {
    tool: String,
    detail: String,
    tool_use_id: String,
}

/// Read the agent's `PreToolUse` payload.
///
/// The fields used are `tool_name`, `tool_input` and `tool_use_id`, verified against the real
/// thing. Anything missing means this is not a payload this build understands, and the answer to
/// that is silence rather than a guess about what somebody is being asked to approve.
fn hook_question(payload: &str) -> Option<Question> {
    let json: serde_json::Value = serde_json::from_str(payload).ok()?;
    let tool = json.get("tool_name")?.as_str()?.to_string();
    let tool_use_id = json.get("tool_use_id")?.as_str()?.to_string();
    let input = json.get("tool_input").cloned().unwrap_or_default();
    let detail = offload_agent::claude::describe_tool_call(&tool, &input);
    Some(Question {
        tool,
        detail,
        tool_use_id,
    })
}

/// Stand in for an MCP server that is on another machine (ADR-0011).
///
/// The agent spawns this exactly as it would spawn a real one, and what it gets is a pipe: every
/// line it writes goes to the daemon, which forwards it to whichever node holds the resource,
/// and every answer comes back the same way. Nothing here understands a word of it, which is the
/// point — an agent's protocol is the agent's (ADR-0004), and a proxy that parsed it would be a
/// second implementation of somebody else's spec.
///
/// A failure prints nothing and exits, which is what an MCP server that cannot start looks like:
/// the agent reports the server as unavailable and carries on with the tools it does have. The
/// alternative — writing an error onto stdout — would be answering the agent in a protocol it is
/// waiting to speak, with something that is not a message in it.
async fn use_resource() -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[derive(Parser, Debug)]
    #[command(name = "offloadd use-resource")]
    struct Args {
        /// Ignored positional: the subcommand word itself.
        #[arg(value_parser = ["use-resource"])]
        _mode: String,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        run: String,
        #[arg(long)]
        service: String,
    }
    let args = Args::parse();

    let stream = tokio::net::UnixStream::connect(&args.socket)
        .await
        .with_context(|| format!("connecting to {}", args.socket.display()))?;
    let (read, mut write) = stream.into_split();
    let mut from_daemon = BufReader::new(read).lines();

    let open = serde_json::to_string(&offload_node::api::Request::UseResource {
        run: args.run.clone(),
        service: args.service.clone(),
    })?;
    write.write_all(open.as_bytes()).await?;
    write.write_all(b"\n").await?;
    write.flush().await?;

    // Nothing is read from the agent until the far end has agreed, so a resource that could not
    // be opened never swallows a request the agent thinks it sent.
    match from_daemon.next_line().await? {
        Some(line) => match serde_json::from_str::<offload_node::api::Response>(&line) {
            Ok(offload_node::api::Response::ResourceOpen { .. }) => {}
            Ok(offload_node::api::Response::Error { message }) => {
                eprintln!("offload: {message}");
                return Ok(());
            }
            _ => return Ok(()),
        },
        None => return Ok(()),
    }

    let mut from_agent = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    loop {
        tokio::select! {
            line = from_agent.next_line() => match line {
                Ok(Some(line)) => {
                    let wrapped =
                        serde_json::to_string(&offload_node::api::Request::ResourceLine { line })?;
                    write.write_all(wrapped.as_bytes()).await?;
                    write.write_all(b"\n").await?;
                    write.flush().await?;
                }
                // The agent closed stdin: it is finished with this server. Dropping the socket
                // is what tells the daemon, which tells the holder, which stops the program.
                Ok(None) | Err(_) => return Ok(()),
            },
            line = from_daemon.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(offload_node::api::Response::ResourceLine { line }) =
                        serde_json::from_str::<offload_node::api::Response>(&line)
                    {
                        stdout.write_all(line.as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                    }
                }
                Ok(None) | Err(_) => return Ok(()),
            },
        }
    }
}

/// Turn the daemon's decision into the agent's hook output, or into nothing.
///
/// `None` for "no decision", which is the important one: the agent's own rules then apply, and
/// they are the rules that applied before any of this existed.
fn hook_response(answer: &offload_node::api::Response) -> Option<String> {
    let offload_node::api::Response::Decision { answer, reason } = answer else {
        return None;
    };
    let decision = match answer {
        offload_core::Answer::Allowed => "allow",
        offload_core::Answer::Denied => "deny",
        offload_core::Answer::Unanswered => return None,
    };
    Some(
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": reason,
            }
        })
        .to_string(),
    )
}

/// One request, one reply, over the daemon's control socket.
///
/// A few lines of its own rather than the CLI's client, because the CLI is a separate binary and
/// this one is the daemon: depending on `offload-cli` from here would be the dependency graph
/// pointing backwards for the sake of thirty lines.
async fn ask_daemon(
    socket: &std::path::Path,
    request: &offload_node::api::Request,
) -> Option<offload_node::api::Response> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::UnixStream::connect(socket).await.ok()?;
    let (read, mut write) = stream.into_split();
    let line = serde_json::to_string(request).ok()?;
    write.write_all(line.as_bytes()).await.ok()?;
    write.write_all(b"\n").await.ok()?;
    write.flush().await.ok()?;

    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<offload_node::api::Response>(&line) {
            // `Done` and anything else this hook has no use for: keep reading until the reply it
            // is waiting for, or the daemon hangs up.
            Ok(offload_node::api::Response::Decision { answer, reason }) => {
                return Some(offload_node::api::Response::Decision { answer, reason })
            }
            Ok(_) | Err(_) => continue,
        }
    }
    None
}
