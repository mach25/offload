//! The agent's event stream, and the turn-boundary logic built on top of it.
//!
//! Claude Code emits one JSON object per line on stdout under
//! `--output-format stream-json`. The shapes here were captured from a real run
//! (`claude 2.1.220`), not inferred — but they are somebody else's wire format and will
//! change, so parsing is deliberately lenient: unknown event types and unknown fields are
//! ignored rather than treated as errors. An adapter that hard-fails on a new event type
//! would break every run the day the agent is upgraded.
//!
//! [`TurnTracker`] is the important part. ADR-0004 says checkpoints happen only at turn
//! boundaries; this is where "turn boundary" stops being prose and becomes a testable
//! predicate.

use serde::Deserialize;
use std::collections::BTreeMap;

/// What the rest of Offload sees. A normalized subset of whatever the agent emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// The agent is up. Carries what it actually resolved, which may differ from what we
    /// asked for — the model in particular.
    Started {
        session_id: String,
        model: String,
        agent_version: String,
        cwd: String,
        tools: Vec<String>,
        permission_mode: String,
    },
    /// Assistant text, as it is produced.
    Text {
        text: String,
    },
    /// The agent invoked a tool. Between this and its result, the run is *not* at a turn
    /// boundary and cannot be checkpointed.
    ToolUse {
        id: String,
        name: String,
    },
    ToolResult {
        id: String,
        is_error: bool,
    },
    /// A complete turn finished with no tool calls outstanding. **The safe checkpoint
    /// point** — the only one, per ADR-0004.
    TurnBoundary {
        turn: u32,
    },
    /// The agent's own view of its rate-limit position. Feeds per-account bid scoring;
    /// this is the real signal the probe's account fingerprint cannot provide.
    RateLimit {
        status: String,
        kind: String,
        resets_at: Option<u64>,
    },
    /// The run ended. Terminal.
    Finished(Outcome),
    /// A line we could parse as JSON but do not model. Kept so nothing is silently lost;
    /// callers normally ignore it.
    Unrecognized {
        event_type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub success: bool,
    pub turns: u32,
    pub stop_reason: Option<String>,
    /// The agent's final message.
    pub result: Option<String>,
    pub duration_ms: Option<u64>,
    /// Cost in micro-dollars. Integer on purpose: this gets summed and compared, and
    /// floats that get summed and compared eventually disagree with themselves.
    pub cost_micro_usd: Option<u64>,
    /// Permission requests the agent made that were denied. Non-empty here usually means
    /// the run was placed with a permission mode it could not work under.
    pub permission_denials: u32,
}

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------

/// One line of `--output-format stream-json`.
///
/// `#[serde(other)]` on the fallback variant is load-bearing: it makes an unrecognized
/// `type` a value rather than a parse failure.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawEvent {
    System(SystemEvent),
    Assistant(MessageEvent),
    User(MessageEvent),
    Result(ResultEvent),
    RateLimitEvent(RateLimitEvent),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct SystemEvent {
    subtype: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    claude_code_version: String,
    #[serde(rename = "permissionMode", default)]
    permission_mode: String,
}

#[derive(Debug, Deserialize)]
struct MessageEvent {
    message: RawMessage,
    /// Set when the event belongs to a subagent rather than the main loop. Subagent
    /// activity must not drive the main turn counter.
    #[serde(default)]
    parent_tool_use_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    ToolUse {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
    },
    ToolResult {
        #[serde(default, rename = "tool_use_id")]
        tool_use_id: String,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct ResultEvent {
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    num_turns: u32,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    permission_denials: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RateLimitEvent {
    rate_limit_info: RateLimitInfo,
}

#[derive(Debug, Deserialize)]
struct RateLimitInfo {
    #[serde(default)]
    status: String,
    #[serde(rename = "rateLimitType", default)]
    kind: String,
    #[serde(rename = "resetsAt", default)]
    resets_at: Option<u64>,
}

/// Convert the agent's float dollars into integer micro-dollars.
///
/// Returns `None` rather than a wrong number for anything that isn't a sane amount —
/// negative, infinite, or absurdly large. Cost gets summed across a fleet and compared
/// against budgets, and a silently truncated value is worse than a missing one.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn dollars_to_micros(dollars: f64) -> Option<u64> {
    let micros = (dollars * 1_000_000.0).round();
    if micros.is_finite() && micros >= 0.0 && micros < u64::MAX as f64 {
        Some(micros as u64)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Turn tracking
// ---------------------------------------------------------------------------

/// Decides when the run is at a turn boundary.
///
/// The rule: a turn is complete when the agent has produced an assistant message and no
/// tool call is outstanding. Mid-tool-call, state lives in the tool — a half-written file,
/// a running subprocess — and the transcript cannot see it, which is exactly why ADR-0004
/// forbids checkpointing there.
///
/// Pure and synchronous, so the interesting sequences are unit tests rather than a
/// live agent run.
#[derive(Debug, Default)]
pub struct TurnTracker {
    /// Tool calls issued but not yet resolved, by id.
    pending: BTreeMap<String, ()>,
    turns: u32,
    /// Whether the agent has said anything since the last boundary. Prevents counting a
    /// boundary before any work happened.
    spoke: bool,
}

impl TurnTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }

    /// Is it safe to checkpoint right now?
    #[must_use]
    pub fn at_boundary(&self) -> bool {
        self.pending.is_empty()
    }

    #[must_use]
    pub fn pending_tools(&self) -> usize {
        self.pending.len()
    }

    /// Feed one line of agent output. Returns the events it produced, in order.
    pub fn observe(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(raw) = serde_json::from_str::<RawEvent>(line) else {
            // Not JSON at all — the agent writing something unexpected to stdout is not
            // our problem to escalate, but it is worth not pretending we saw nothing.
            return vec![AgentEvent::Unrecognized {
                event_type: "malformed".to_string(),
            }];
        };
        self.observe_raw(raw)
    }

    fn observe_raw(&mut self, raw: RawEvent) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match raw {
            RawEvent::System(sys) if sys.subtype == "init" => {
                out.push(AgentEvent::Started {
                    session_id: sys.session_id,
                    model: sys.model,
                    agent_version: sys.claude_code_version,
                    cwd: sys.cwd,
                    tools: sys.tools,
                    permission_mode: sys.permission_mode,
                });
            }
            RawEvent::System(_) => {}

            RawEvent::Assistant(ev) => {
                // Subagent output rides the same stream. It must not move the main
                // loop's turn counter, or a fan-out of subagents looks like a dozen
                // checkpointable boundaries that never happened.
                if ev.parent_tool_use_id.is_some() {
                    return out;
                }
                let mut issued_tools = false;
                for block in ev.message.content {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            self.spoke = true;
                            out.push(AgentEvent::Text { text });
                        }
                        ContentBlock::ToolUse { id, name } => {
                            self.spoke = true;
                            issued_tools = true;
                            self.pending.insert(id.clone(), ());
                            out.push(AgentEvent::ToolUse { id, name });
                        }
                        _ => {}
                    }
                }
                // A turn ends when the agent speaks without reaching for a tool.
                if !issued_tools && self.spoke && self.pending.is_empty() {
                    self.turns += 1;
                    self.spoke = false;
                    out.push(AgentEvent::TurnBoundary { turn: self.turns });
                }
            }

            RawEvent::User(ev) => {
                if ev.parent_tool_use_id.is_some() {
                    return out;
                }
                for block in ev.message.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        is_error,
                    } = block
                    {
                        self.pending.remove(&tool_use_id);
                        out.push(AgentEvent::ToolResult {
                            id: tool_use_id,
                            is_error,
                        });
                    }
                }
            }

            RawEvent::RateLimitEvent(ev) => out.push(AgentEvent::RateLimit {
                status: ev.rate_limit_info.status,
                kind: ev.rate_limit_info.kind,
                resets_at: ev.rate_limit_info.resets_at,
            }),

            RawEvent::Result(res) => {
                // The agent's own turn count is authoritative — ours is a local estimate
                // used for live boundary detection, and the two can disagree if the
                // stream shape surprises us.
                self.turns = res.num_turns.max(self.turns);
                out.push(AgentEvent::Finished(Outcome {
                    success: !res.is_error && res.subtype == "success",
                    turns: res.num_turns,
                    stop_reason: res.stop_reason,
                    result: res.result,
                    duration_ms: res.duration_ms,
                    cost_micro_usd: res.total_cost_usd.and_then(dollars_to_micros),
                    permission_denials: u32::try_from(res.permission_denials.len())
                        .unwrap_or(u32::MAX),
                }));
            }

            RawEvent::Other => out.push(AgentEvent::Unrecognized {
                event_type: "unknown".to_string(),
            }),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant_text(text: &str) -> String {
        format!(
            r#"{{"type":"assistant","parent_tool_use_id":null,"message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    fn assistant_tool(id: &str, name: &str) -> String {
        format!(
            r#"{{"type":"assistant","parent_tool_use_id":null,"message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}"}}]}}}}"#
        )
    }

    fn tool_result(id: &str) -> String {
        format!(
            r#"{{"type":"user","parent_tool_use_id":null,"message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":false}}]}}}}"#
        )
    }

    #[test]
    fn parses_the_real_init_event() {
        // Captured verbatim from `claude 2.1.220`.
        let line = r#"{"type":"system","subtype":"init","session_id":"11111111-2222-3333-4444-555555555555","model":"claude-haiku-4-5","cwd":"/tmp/work","tools":["Bash","Read"],"claude_code_version":"2.1.220","permissionMode":"default","slash_commands":["init"],"mcp_servers":[]}"#;
        let mut t = TurnTracker::new();
        let events = t.observe(line);
        assert_eq!(
            events,
            vec![AgentEvent::Started {
                session_id: "11111111-2222-3333-4444-555555555555".into(),
                model: "claude-haiku-4-5".into(),
                agent_version: "2.1.220".into(),
                cwd: "/tmp/work".into(),
                tools: vec!["Bash".into(), "Read".into()],
                permission_mode: "default".into(),
            }]
        );
    }

    #[test]
    fn a_text_only_reply_is_a_turn_boundary() {
        let mut t = TurnTracker::new();
        let events = t.observe(&assistant_text("done"));
        assert!(events.contains(&AgentEvent::TurnBoundary { turn: 1 }));
        assert!(t.at_boundary());
    }

    #[test]
    fn mid_tool_call_is_never_a_boundary() {
        // The core safety property of ADR-0004: state lives in the tool, not the
        // transcript, so this window must never look checkpointable.
        let mut t = TurnTracker::new();
        t.observe(&assistant_tool("toolu_1", "Bash"));

        assert!(!t.at_boundary(), "a pending tool call is not a boundary");
        assert_eq!(t.pending_tools(), 1);
        assert_eq!(t.turns(), 0);

        t.observe(&tool_result("toolu_1"));
        assert!(t.at_boundary());
        assert_eq!(t.turns(), 0, "resolving a tool is not itself a turn");

        let events = t.observe(&assistant_text("finished"));
        assert!(events.contains(&AgentEvent::TurnBoundary { turn: 1 }));
    }

    #[test]
    fn parallel_tool_calls_all_have_to_resolve() {
        let mut t = TurnTracker::new();
        let parallel = r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[
            {"type":"tool_use","id":"a","name":"Read"},
            {"type":"tool_use","id":"b","name":"Grep"}]}}"#;
        t.observe(parallel);
        assert_eq!(t.pending_tools(), 2);

        t.observe(&tool_result("a"));
        assert!(!t.at_boundary(), "one of two resolved is still mid-turn");

        t.observe(&tool_result("b"));
        assert!(t.at_boundary());
    }

    #[test]
    fn subagent_output_does_not_advance_the_main_turn_counter() {
        // Subagent activity shares the stream. Counting it would manufacture boundaries
        // the main loop never reached.
        let mut t = TurnTracker::new();
        let sub = r#"{"type":"assistant","parent_tool_use_id":"toolu_parent","message":{"content":[{"type":"text","text":"subagent thinking"}]}}"#;
        let events = t.observe(sub);
        assert!(events.is_empty());
        assert_eq!(t.turns(), 0);
    }

    #[test]
    fn unknown_event_types_are_survivable() {
        // The agent will add event types. That must not break a run.
        let mut t = TurnTracker::new();
        let events = t.observe(r#"{"type":"some_future_event","payload":{"a":1}}"#);
        assert_eq!(
            events,
            vec![AgentEvent::Unrecognized {
                event_type: "unknown".into()
            }]
        );

        assert_eq!(t.observe("not json at all").len(), 1);
        assert!(t.observe("   ").is_empty());
    }

    #[test]
    fn extra_fields_on_known_events_are_ignored() {
        let mut t = TurnTracker::new();
        let with_extras = r#"{"type":"assistant","parent_tool_use_id":null,"request_id":"req_1","uuid":"u","timestamp":"t","message":{"id":"m","role":"assistant","model":"x","content":[{"type":"text","text":"hi"}]}}"#;
        assert!(t
            .observe(with_extras)
            .contains(&AgentEvent::TurnBoundary { turn: 1 }));
    }

    #[test]
    fn parses_the_real_result_event() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"num_turns":3,"stop_reason":"end_turn","result":"ok","duration_ms":4200,"total_cost_usd":0.0123,"permission_denials":[],"usage":{}}"#;
        let mut t = TurnTracker::new();
        let events = t.observe(line);
        let AgentEvent::Finished(outcome) = &events[0] else {
            panic!("expected Finished, got {events:?}");
        };
        assert!(outcome.success);
        assert_eq!(outcome.turns, 3);
        assert_eq!(outcome.cost_micro_usd, Some(12_300));
        assert_eq!(outcome.result.as_deref(), Some("ok"));
    }

    #[test]
    fn an_errored_result_is_not_a_success() {
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"permission_denials":[{"tool":"Bash"}]}"#;
        let mut t = TurnTracker::new();
        let AgentEvent::Finished(outcome) = &t.observe(line)[0] else {
            panic!("expected Finished");
        };
        assert!(!outcome.success);
        assert_eq!(outcome.permission_denials, 1);
    }

    #[test]
    fn rate_limit_events_are_surfaced() {
        // This is the real per-account pressure signal the probe can't synthesize.
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1785006000,"rateLimitType":"five_hour"}}"#;
        let mut t = TurnTracker::new();
        assert_eq!(
            t.observe(line),
            vec![AgentEvent::RateLimit {
                status: "allowed".into(),
                kind: "five_hour".into(),
                resets_at: Some(1_785_006_000),
            }]
        );
    }

    #[test]
    fn a_full_session_counts_turns_the_way_the_agent_does() {
        let mut t = TurnTracker::new();
        t.observe(&assistant_text("let me look"));
        t.observe(&assistant_tool("t1", "Read"));
        t.observe(&tool_result("t1"));
        t.observe(&assistant_text("found it"));
        t.observe(&assistant_tool("t2", "Edit"));
        t.observe(&tool_result("t2"));
        t.observe(&assistant_text("done"));
        assert_eq!(t.turns(), 3);
        assert!(t.at_boundary());
    }
}
