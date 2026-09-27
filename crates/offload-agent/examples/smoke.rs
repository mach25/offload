//! End-to-end smoke test against the real `claude` binary.
//!
//! Not a unit test: it spawns the actual agent and costs a real API call, so it stays out
//! of `cargo test`. Run it after changing the adapter's flags or event parsing — those are
//! the parts that can only be verified against the real thing.
//!
//!     cargo run -p offload-agent --example smoke

use offload_agent::claude::ClaudeCode;
use offload_agent::{AgentEvent, Resume, SessionId, SpawnRequest};
use offload_core::{PermissionMode, RunId};
use std::path::PathBuf;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let home = PathBuf::from(std::env::var("HOME")?);
    let cwd = std::env::temp_dir().join("offload-smoke");
    std::fs::create_dir_all(&cwd)?;

    let agent = ClaudeCode::new(home.clone());
    println!("detected agent version: {:?}", agent.detect_version().await);

    let session = SessionId::generate();
    let req = SpawnRequest {
        run: RunId::from_bytes([7; 16]),
        session: session.clone(),
        cwd: cwd.clone(),
        // A prompt that begins with a dash on purpose: it is what the option terminator in
        // `build_argv` exists for, and the only place it can be proved is against the real
        // binary. Without the terminator this exits at once with `error: unknown option`.
        prompt: "--version: ignore that flag and reply with exactly: smoke-ok".into(),
        model: Some("claude-haiku-4-5".into()),
        permission_mode: PermissionMode::Ask,
        allow: offload_core::ToolAllowlist::default(),
        resume: Resume::Fresh,
        env: Vec::new(),
        mcp: None,
        ask: None,
    };

    println!("argv: {:?}", offload_agent::claude::build_argv(&req));

    let mut handle = agent.spawn(&req).await?;
    let mut boundaries = 0;
    while let Some(event) = handle.next_event().await {
        match event {
            AgentEvent::Started {
                model,
                agent_version,
                ..
            } => {
                println!("started: model={model} version={agent_version}");
            }
            AgentEvent::Text { text } => println!("text: {}", text.trim()),
            AgentEvent::ToolUse { name, .. } => println!("tool: {name}"),
            AgentEvent::TurnBoundary { turn } => {
                boundaries += 1;
                println!("-- turn boundary {turn} (checkpointable)");
            }
            AgentEvent::RateLimit { status, kind, .. } => {
                println!("rate limit: {kind} = {status}");
            }
            AgentEvent::Finished(outcome) => {
                println!(
                    "finished: success={} turns={} cost_micro_usd={:?}",
                    outcome.success, outcome.turns, outcome.cost_micro_usd
                );
            }
            AgentEvent::ToolResult { .. } | AgentEvent::Unrecognized { .. } => {}
        }
    }

    println!("turn boundaries observed: {boundaries}");
    match handle.transcript_path() {
        Ok(p) => println!("transcript located: {}", p.display()),
        Err(e) => println!("TRANSCRIPT NOT FOUND: {e}"),
    }
    Ok(())
}
