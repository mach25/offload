//! A question an agent is blocked on, as somebody else sees it (ADR-0017).
//!
//! The pure half of the approval channel: what a waiting question *looks like* to whoever asks
//! what is waiting. The registry that holds them and the wait itself belong to the node running
//! the agent — a blocked process is not a fact that can be gossiped, because it stops being true
//! the moment somebody answers.
//!
//! Here rather than in the daemon for [`crate::LogEvent`]'s reason: it crosses the wire, in two
//! directions that must agree. A laptop asking the desktop what it is waiting for, and the same
//! answer coming back out of `offload asks`, are one shape — and two copies of it would drift the
//! first time a field was added.

use crate::id::{NodeId, RunId};
use crate::time::Millis;
use serde::{Deserialize, Serialize};

/// How many times one run may stop and ask a person (ADR-0017).
///
/// A count rather than the flag this started as, because widening the channel to cover *edits*
/// forced the question the ADR left open: how many questions is a person willing to answer for
/// one run? Under the product default only a command or a fetch the run has no grant for asks
/// anything, and that is a handful a night. Under [`PermissionMode::Ask`] the agent gates every
/// edit as well, so an ordinary coding run would put twenty or thirty questions to somebody's
/// phone — and the twentieth is not read, which makes the first nineteen worse than useless.
///
/// So the budget is what makes the wider list honest. It bounds the *interruption*, and it is a
/// ceiling rather than an expectation: nearly every run spends none of it.
///
/// Spending it is not a refusal. A run past its budget asks nothing more and its calls are
/// decided by the agent's own rules — which is exactly what nobody-answering does, and what every
/// run in the fleet did before this channel existed. The failure mode of running out is therefore
/// the behaviour we already ship, which is the property that makes a budget safe to have at all.
///
/// Counted on the **run** rather than on a leg of it ([`RunProgress`]), so a migration continues
/// the count instead of handing somebody a fresh twenty.
///
/// [`PermissionMode::Ask`]: crate::PermissionMode
/// [`RunProgress`]: crate::RunProgress
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskPolicy {
    /// Never stop: a call the run does not already permit is denied headless, as it always was.
    #[default]
    Never,
    /// Stop and ask, up to this many times over the whole life of the run.
    UpTo { questions: u32 },
}

/// What `--ask` means with no number after it.
///
/// Roughly one run's worth of somebody's attention. Chosen from the shape of the two modes rather
/// than from nothing: under `AcceptEdits` a run asks about the odd ungranted command and never
/// comes near this, and under `Ask` it is about what a twenty-turn coding run edits — so the
/// budget bites exactly where the questions would otherwise have become noise.
pub const DEFAULT_ASK_BUDGET: u32 = 20;

impl AskPolicy {
    /// May this run stop at all? What the spawn path asks, and what submit checks before
    /// deciding whether `Ask` is answerable on this node.
    #[must_use]
    pub const fn enabled(self) -> bool {
        matches!(self, AskPolicy::UpTo { .. })
    }

    #[must_use]
    pub const fn budget(self) -> u32 {
        match self {
            AskPolicy::Never => 0,
            AskPolicy::UpTo { questions } => questions,
        }
    }

    /// Is there room for one more question, given how many this run has already put to somebody?
    #[must_use]
    pub const fn may_ask(self, asked: u32) -> bool {
        asked < self.budget()
    }
}

impl std::fmt::Display for AskPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskPolicy::Never => f.write_str("never"),
            AskPolicy::UpTo { questions: 1 } => f.write_str("up to 1 question"),
            AskPolicy::UpTo { questions } => write!(f, "up to {questions} questions"),
        }
    }
}

/// One question, waiting.
///
/// Durations rather than rendered strings, and they are computed **where the wait is happening**:
/// the holder knows when it asked and how long its own patience is, and a caller that subtracted
/// timestamps across two machines would be reporting clock skew as urgency. Rendering is the
/// display's job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAsk {
    pub run: RunId,
    /// Which node's agent is waiting. `None` means the node that produced this answer — filled
    /// in by whoever collected it, because a node does not need to be told its own name.
    #[serde(default)]
    pub node: Option<NodeId>,
    /// What to call that node when telling somebody. Filled in beside the id and for the same
    /// reason a delivery route carries one: the id is the identity and the name is for reading,
    /// and only the collector's view knows the second.
    #[serde(default)]
    pub node_name: Option<String>,
    /// The agent's own id for the tool call. What an answer is addressed to, and what
    /// disambiguates a run blocked on several calls at once.
    pub tool_use_id: String,
    pub tool: String,
    /// One line describing what the agent wants to do.
    pub detail: String,
    pub waiting: Millis,
    /// How long is left before nobody-answering decides it.
    pub left: Millis,
}

impl PendingAsk {
    /// Where this is waiting, for a person reading one line.
    ///
    /// "here" for the machine being typed at, which is the common case and the one where a node
    /// id would be noise. Otherwise the name, falling back to the id: discovery hands over a key
    /// before it hands over a name, and a blank column reads as a bug rather than as a node
    /// nobody has met yet.
    #[must_use]
    pub fn where_it_waits(&self) -> String {
        match (&self.node_name, self.node) {
            (Some(name), _) => name.clone(),
            (None, Some(node)) => node.short(),
            (None, None) => "here".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_bounds_the_interruption_and_running_out_is_not_a_refusal() {
        let policy = AskPolicy::UpTo { questions: 2 };
        assert!(policy.may_ask(0));
        assert!(policy.may_ask(1));
        // The third question is not asked. What happens to that tool call is the agent's own
        // rules, which is the same thing nobody-answering leaves behind.
        assert!(!policy.may_ask(2));
        assert!(!policy.may_ask(9));
    }

    #[test]
    fn a_run_that_never_asked_has_no_budget_to_spend() {
        assert!(!AskPolicy::Never.enabled());
        assert!(!AskPolicy::Never.may_ask(0));
        assert_eq!(AskPolicy::Never.budget(), 0);
        assert!(AskPolicy::UpTo { questions: 1 }.enabled());
    }

    #[test]
    fn an_ask_policy_round_trips() {
        for policy in [
            AskPolicy::Never,
            AskPolicy::UpTo {
                questions: DEFAULT_ASK_BUDGET,
            },
        ] {
            let json = serde_json::to_string(&policy).expect("encode");
            assert_eq!(
                serde_json::from_str::<AskPolicy>(&json).expect("decode"),
                policy
            );
        }
        // The shape a peer will actually see, written out, because this travels on a spec.
        assert_eq!(
            serde_json::to_string(&AskPolicy::Never).expect("encode"),
            r#""never""#
        );
        assert_eq!(
            serde_json::to_string(&AskPolicy::UpTo { questions: 3 }).expect("encode"),
            r#"{"up_to":{"questions":3}}"#
        );
    }

    #[test]
    fn a_policy_says_itself_the_way_a_person_would_read_it() {
        assert_eq!(AskPolicy::Never.to_string(), "never");
        assert_eq!(
            AskPolicy::UpTo { questions: 1 }.to_string(),
            "up to 1 question"
        );
        assert_eq!(
            AskPolicy::UpTo { questions: 4 }.to_string(),
            "up to 4 questions"
        );
    }

    fn ask() -> PendingAsk {
        PendingAsk {
            run: RunId::from_bytes([3; 16]),
            node: None,
            node_name: None,
            tool_use_id: "toolu_1".into(),
            tool: "Bash".into(),
            detail: "cargo build".into(),
            waiting: Millis(12_000),
            left: Millis(288_000),
        }
    }

    #[test]
    fn a_question_on_this_machine_is_here_and_one_elsewhere_is_named() {
        assert_eq!(ask().where_it_waits(), "here");
        let named = PendingAsk {
            node: Some(NodeId::from_bytes([9; 32])),
            node_name: Some("desktop".into()),
            ..ask()
        };
        assert_eq!(named.where_it_waits(), "desktop");
        // A node this fleet has a key for and no name yet: the id, not an empty column.
        let unnamed = PendingAsk {
            node: Some(NodeId::from_bytes([9; 32])),
            node_name: None,
            ..ask()
        };
        assert_eq!(
            unnamed.where_it_waits(),
            NodeId::from_bytes([9; 32]).short()
        );
    }

    #[test]
    fn it_round_trips_with_the_node_absent() {
        // The common case on the wire is a holder describing its own questions, which sends no
        // node at all — so the absence has to decode rather than fail.
        let json = serde_json::to_string(&ask()).expect("encode");
        assert_eq!(
            serde_json::from_str::<PendingAsk>(&json).expect("decode"),
            ask()
        );
        // Hand-written, with `run` as the array of bytes a `RunId` actually encodes as — the
        // same shape that makes `Payload` exist in the delivery plane.
        let without: PendingAsk = serde_json::from_str(
            r#"{"run":[3,3,3,3,3,3,3,3,3,3,3,3,3,3,3,3],"tool_use_id":"t","tool":"Bash",
                "detail":"d","waiting":1,"left":2}"#,
        )
        .expect("decode without a node");
        assert_eq!(without.node, None);
    }
}
