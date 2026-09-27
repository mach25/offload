//! Properties of the command line this adapter builds.
//!
//! `build_argv` has a unit test per flag, and the question those cannot answer is whether
//! some *combination* loses one — the argv is a flat list with two ordering rules on it
//! (`--allowedTools` is variadic, the prompt is positional), and both of them are about what
//! follows what.
//!
//! Everything asserted here was measured against `claude 2.1.238`. The agent's own parser is
//! not ours, so nothing below is a claim about how command lines ought to work.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use offload_agent::claude::build_argv;
use offload_agent::{AskHook, Resume, SessionId, SpawnRequest};
use offload_core::{PermissionMode, RunId, ToolAllowlist};
use proptest::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

/// Prompts an operator can actually type, including the ones that look like flags.
fn prompt() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("add tests for the parser".to_string()),
        Just("--version is what I want documented".to_string()),
        Just("-p should be explained in the README".to_string()),
        Just("--".to_string()),
        Just("--allowedTools Bash(rm:*)".to_string()),
        Just(String::new()),
        Just("two\nlines".to_string()),
    ]
}

fn allowlist() -> impl Strategy<Value = ToolAllowlist> {
    prop::collection::vec(
        prop_oneof![
            Just("Bash(cargo test:*)"),
            Just("Edit"),
            Just("Read"),
            Just("mcp__email__send"),
        ],
        0..4,
    )
    .prop_map(|patterns| ToolAllowlist::parse(patterns).expect("valid patterns"))
}

fn request() -> impl Strategy<Value = SpawnRequest> {
    (
        prompt(),
        prop::option::of(prop_oneof![
            Just("claude-opus-5".to_string()),
            Just("claude-sonnet-5".to_string())
        ]),
        prop_oneof![
            Just(PermissionMode::Ask),
            Just(PermissionMode::AcceptEdits),
            Just(PermissionMode::Full),
        ],
        allowlist(),
        prop_oneof![
            Just(Resume::Fresh),
            (prompt(), any::<bool>()).prop_map(|(prompt, fork)| Resume::Continue { prompt, fork }),
        ],
        prop::option::of(Just(PathBuf::from("/state/mcp/run.json"))),
        any::<bool>(),
    )
        .prop_map(
            |(prompt, model, permission_mode, allow, resume, mcp, ask)| SpawnRequest {
                run: RunId::from_bytes([1; 16]),
                session: SessionId("11111111-2222-3333-4444-555555555555".into()),
                cwd: PathBuf::from("/work/repo"),
                prompt,
                model,
                permission_mode,
                allow,
                resume,
                env: Vec::new(),
                mcp,
                ask: ask.then(|| AskHook {
                    command: PathBuf::from("/opt/offload/offloadd"),
                    args: vec!["ask-hook".into()],
                    timeout: Duration::from_secs(310),
                    tools: vec!["Bash".into()],
                }),
            },
        )
}

proptest! {
    /// The prompt the run was submitted with is what the agent is asked, whatever it looks
    /// like and whatever else is on the line.
    ///
    /// Measured on 2.1.238: without the terminator a prompt beginning with a dash exits at
    /// once with `error: unknown option`, and asking an agent about a CLI flag is an ordinary
    /// thing to ask. With it the same run starts, and the grants before it still apply.
    #[test]
    fn the_prompt_is_the_last_word_and_the_parser_stops_before_it(req in request()) {
        let argv = build_argv(&req);
        let expected = match &req.resume {
            Resume::Fresh => req.prompt.clone(),
            Resume::Continue { prompt, .. } => prompt.clone(),
        };

        prop_assert_eq!(argv.last(), Some(&expected));
        prop_assert_eq!(argv[argv.len() - 2].as_str(), "--");
        prop_assert_eq!(
            argv.iter().filter(|a| *a == "--").count(),
            if expected == "--" { 2 } else { 1 },
            "{:?}", argv
        );
    }

    /// Nothing a request asked for is dropped by the presence of anything else. One
    /// assertion per flag, over every combination rather than one at a time.
    #[test]
    fn every_flag_the_request_asked_for_survives_the_combination(req in request()) {
        let argv = build_argv(&req);
        let value_of = |flag: &str| {
            argv.iter()
                .position(|a| a == flag)
                .and_then(|i| argv.get(i + 1))
                .cloned()
        };

        prop_assert!(argv.contains(&"--print".to_string()));
        prop_assert!(argv.contains(&"--verbose".to_string()));
        prop_assert!(argv.contains(&"--strict-mcp-config".to_string()), "{:?}", argv);
        let output_format = value_of("--output-format");
        prop_assert_eq!(output_format.as_deref(), Some("stream-json"));
        prop_assert!(value_of("--permission-mode").is_some());
        prop_assert_eq!(value_of("--model"), req.model.clone());
        prop_assert_eq!(
            value_of("--mcp-config"),
            req.mcp.as_ref().map(|p| p.display().to_string())
        );
        prop_assert_eq!(value_of("--settings").is_some(), req.ask.is_some());

        match &req.resume {
            Resume::Fresh => {
                let session = value_of("--session-id");
                prop_assert_eq!(session.as_deref(), Some(req.session.0.as_str()));
                prop_assert!(!argv.contains(&"--resume".to_string()));
                prop_assert!(!argv.contains(&"--fork-session".to_string()));
            }
            Resume::Continue { fork, .. } => {
                let session = value_of("--resume");
                prop_assert_eq!(session.as_deref(), Some(req.session.0.as_str()));
                prop_assert!(!argv.contains(&"--session-id".to_string()));
                prop_assert_eq!(argv.contains(&"--fork-session".to_string()), *fork);
            }
        }
    }

    /// The variadic flag's own rule: every pattern reaches it, in order, and nothing else
    /// does. A pattern lost here is a tool the run is silently denied; a stray word gained is
    /// a grant nobody made.
    #[test]
    fn the_allowlist_ends_the_flags_and_keeps_every_pattern(req in request()) {
        let argv = build_argv(&req);
        let patterns = req.allow.to_args();

        match argv.iter().position(|a| a == "--allowedTools") {
            None => prop_assert!(patterns.is_empty(), "{:?}", argv),
            Some(at) => {
                let terminator = argv.len() - 2;
                prop_assert_eq!(&argv[at + 1..terminator], patterns.as_slice());
            }
        }
    }
}
