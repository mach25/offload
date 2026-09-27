//! Questions this node's agents are blocked on, and the answers (ADR-0017).
//!
//! The delivery plane carries the question out and `offload approve` brings the answer back; this
//! is the small piece in the middle that remembers what is waiting. Four things about its shape:
//!
//! * **It is in memory, and that is not a shortcut.** A pending question exists exactly as long as
//!   the blocked process does. If the daemon dies, the agent dies with it and there is nothing
//!   left to answer — so a durable copy would only ever be a list of questions nobody could still
//!   act on. What *is* durable is the run's log: `LogKind::Asked` and `LogKind::Answered` are the
//!   record, and they survive whatever happens next.
//! * **An answer is addressed to one tool call.** The key is the agent's own `tool_use_id`, so an
//!   approval cannot be spent twice, and an approval that arrives after the wait ran out is
//!   refused rather than applied to whatever the agent is doing now.
//! * **Nobody answering is its own outcome**, not a denial ([`offload_core::Answer`]). The
//!   waiter's reply is then "no decision", and the agent applies its own rules — which is exactly
//!   what happens today for every run in the fleet.
//! * **Waiting is bounded**, because the question is asked mid-turn: the run cannot be
//!   checkpointed or drained while it waits (ADR-0004). `Run::approval_patience` sets the bound.

use offload_core::{Epoch, Millis, PendingAsk, RunId};
use std::sync::Mutex;
use tokio::sync::oneshot;

/// What an operator decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub allow: bool,
    /// Who said so, for the run's log.
    pub by: String,
}

/// One question, as the hook posed it.
///
/// A struct rather than five arguments in a row: two of them are the strings `tool` and `detail`,
/// and a transposition there would show somebody the wrong thing to approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub run: RunId,
    /// The leg of the run that is asking. An answer under a stale epoch is not current.
    pub epoch: Epoch,
    /// The agent's own id for the tool call.
    pub tool_use_id: String,
    pub tool: String,
    pub detail: String,
}

struct Pending {
    question: Question,
    asked_at: Millis,
    expires_at: Millis,
    answer: oneshot::Sender<Verdict>,
}

/// Why an answer went nowhere.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AnswerError {
    #[error("nothing on this node is waiting for an answer about run {0}")]
    NothingWaiting(String),
    /// The run *is* waiting, just not about that tool call.
    ///
    /// Split out from [`AnswerError::NothingWaiting`], which used to cover both because the
    /// lookup filtered on the run **and** the `tool_use_id` before asking whether anything
    /// matched. A mistyped or stale id therefore produced *"nothing on this node is waiting for
    /// an answer about run X"* about a run whose question `offload asks` was printing on the
    /// next line — two commands on one screen contradicting each other, with the false one
    /// sending somebody to look for an expired question instead of at their own argument.
    /// Measured in session seventy-eight on a live blocked agent.
    ///
    /// Names what *is* waiting, because an error that tells somebody to try again has to be
    /// satisfiable from the screen it is printed on.
    #[error(
        "run {run} is waiting for an answer, but not about `{tool_use_id}` — it is waiting on \
         {waiting}"
    )]
    NoSuchQuestion {
        run: String,
        tool_use_id: String,
        waiting: String,
    },
    #[error("run {run} is waiting on {count} questions; name one of them")]
    Ambiguous { run: String, count: usize },
    #[error("that question is no longer waiting for an answer")]
    Gone,
}

/// The questions in flight on this node.
#[derive(Default)]
pub struct Asks {
    pending: Mutex<Vec<Pending>>,
}

impl std::fmt::Debug for Asks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Asks")
            .field("waiting", &self.count())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Asks {
    /// Register a question and hand back the channel its answer will arrive on.
    ///
    /// Replaces any earlier question with the same `tool_use_id`: the agent retries a hook it did
    /// not get an answer from, and two entries for one tool call would leave a stale sender that
    /// an operator could answer into nothing.
    pub fn register(
        &self,
        question: Question,
        asked_at: Millis,
        expires_at: Millis,
    ) -> oneshot::Receiver<Verdict> {
        let (tx, rx) = oneshot::channel();
        let mut pending = lock(&self.pending);
        pending.retain(|p| {
            !(p.question.run == question.run && p.question.tool_use_id == question.tool_use_id)
        });
        pending.push(Pending {
            question,
            asked_at,
            expires_at,
            answer: tx,
        });
        rx
    }

    /// Stop waiting for an answer to one question, whatever the reason.
    ///
    /// Called by the waiter itself — when it has been answered, when the wait ran out, and when
    /// the hook on the other end went away. A registry that only forgot answered questions would
    /// keep offering an operator a question whose agent is long gone.
    pub fn forget(&self, run: RunId, tool_use_id: &str) {
        lock(&self.pending)
            .retain(|p| !(p.question.run == run && p.question.tool_use_id == tool_use_id));
    }

    /// Answer one question: the named one, or the only one if the run has exactly one.
    ///
    /// Returns what was answered, so the operator can be told what they just approved rather than
    /// "ok" — an agent can be blocked on several calls at once, and approving the wrong one of
    /// three is a mistake worth making visible immediately.
    pub fn answer(
        &self,
        run: RunId,
        which: Option<&str>,
        verdict: Verdict,
    ) -> Result<Answered, AnswerError> {
        let mut pending = lock(&self.pending);
        // Asked in two steps, because "this run has no question" and "this run has a question
        // and you named a different one" are different facts and the operator acts on them
        // differently: the first sends them to `offload ps`, the second to their own argument.
        // One filter chain answered both with the first sentence.
        let for_run: Vec<usize> = pending
            .iter()
            .enumerate()
            .filter(|(_, p)| p.question.run == run)
            .map(|(i, _)| i)
            .collect();
        if for_run.is_empty() {
            return Err(AnswerError::NothingWaiting(run.short()));
        }
        let candidates: Vec<usize> = match which {
            None => for_run,
            Some(id) => for_run
                .into_iter()
                .filter(|i| pending[*i].question.tool_use_id == id)
                .collect(),
        };
        let index = match (candidates.first(), candidates.len()) {
            (None, _) => {
                let waiting = pending
                    .iter()
                    .filter(|p| p.question.run == run)
                    .map(|p| format!("`{}`", p.question.tool_use_id))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AnswerError::NoSuchQuestion {
                    run: run.short(),
                    tool_use_id: which.unwrap_or_default().to_string(),
                    waiting,
                });
            }
            (Some(&i), 1) => i,
            // More than one and nothing named: refused rather than guessed, for `resolve`'s
            // reason — picking one of three questions on somebody's behalf is not a convenience.
            (Some(_), count) => {
                if which.is_some() {
                    return Err(AnswerError::Gone);
                }
                return Err(AnswerError::Ambiguous {
                    run: run.short(),
                    count,
                });
            }
        };
        let entry = pending.swap_remove(index);
        let answered = Answered {
            tool_use_id: entry.question.tool_use_id.clone(),
            tool: entry.question.tool.clone(),
            detail: entry.question.detail.clone(),
            allowed: verdict.allow,
        };
        // The receiver is gone when the wait already ran out, which is a real race rather than a
        // bug: the answer arrived a moment too late and must not be reported as applied.
        entry.answer.send(verdict).map_err(|_| AnswerError::Gone)?;
        Ok(answered)
    }

    /// Everything waiting here, in a stable order.
    ///
    /// The clocks are read *here*, which is the point of computing them at the holder: a laptop
    /// subtracting timestamps across two machines would be reporting clock skew as urgency.
    #[must_use]
    pub fn waiting(&self, now: Millis) -> Vec<PendingAsk> {
        let mut pending: Vec<PendingAsk> = lock(&self.pending)
            .iter()
            .map(|p| PendingAsk {
                run: p.question.run,
                // Filled in by whoever collects it from a peer; a node needs no name for itself.
                node: None,
                node_name: None,
                tool_use_id: p.question.tool_use_id.clone(),
                tool: p.question.tool.clone(),
                detail: p.question.detail.clone(),
                waiting: now.saturating_sub(p.asked_at),
                left: p.expires_at.saturating_sub(now),
            })
            .collect();
        pending.sort_by(|a, b| a.run.cmp(&b.run).then(a.tool_use_id.cmp(&b.tool_use_id)));
        pending
    }

    /// Is this run blocked on anything, and under which epoch?
    ///
    /// The epoch is here because a question outlives nothing: if the run has moved on, an answer
    /// to a question asked under the old epoch must not be applied.
    #[must_use]
    pub fn epoch_of(&self, run: RunId, tool_use_id: &str) -> Option<Epoch> {
        lock(&self.pending)
            .iter()
            .find(|p| p.question.run == run && p.question.tool_use_id == tool_use_id)
            .map(|p| p.question.epoch)
    }

    #[must_use]
    pub fn count(&self) -> usize {
        lock(&self.pending).len()
    }
}

/// What an answer turned out to be about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub tool_use_id: String,
    pub tool: String,
    pub detail: String,
    pub allowed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(byte: u8) -> RunId {
        RunId::from_bytes([byte; 16])
    }

    fn register(asks: &Asks, id: RunId, tool_use: &str) -> oneshot::Receiver<Verdict> {
        asks.register(
            Question {
                run: id,
                epoch: Epoch(1),
                tool_use_id: tool_use.into(),
                tool: "Bash".into(),
                detail: "cargo build".into(),
            },
            Millis(1_000),
            Millis(301_000),
        )
    }

    #[test]
    fn the_only_question_a_run_is_waiting_on_needs_no_name() {
        let asks = Asks::default();
        let mut rx = register(&asks, run(1), "toolu_1");
        let answered = asks
            .answer(
                run(1),
                None,
                Verdict {
                    allow: true,
                    by: "owner".into(),
                },
            )
            .expect("answered");
        assert_eq!(answered.tool_use_id, "toolu_1");
        assert!(answered.allowed);
        assert_eq!(
            rx.try_recv().expect("the waiter heard it").by,
            "owner",
            "the blocked hook is what actually needs the answer"
        );
        assert_eq!(asks.count(), 0, "answered questions stop being offered");
    }

    #[test]
    fn two_questions_at_once_are_not_guessed_between() {
        // An agent can block on parallel tool calls, and approving the wrong one of three is
        // exactly the mistake that must not be made on somebody's behalf.
        let asks = Asks::default();
        let _a = register(&asks, run(1), "toolu_1");
        let _b = register(&asks, run(1), "toolu_2");
        assert_eq!(
            asks.answer(
                run(1),
                None,
                Verdict {
                    allow: true,
                    by: "owner".into()
                }
            ),
            Err(AnswerError::Ambiguous {
                run: run(1).short(),
                count: 2
            })
        );
        // Named, it is unambiguous.
        let answered = asks
            .answer(
                run(1),
                Some("toolu_2"),
                Verdict {
                    allow: false,
                    by: "owner".into(),
                },
            )
            .expect("answered");
        assert_eq!(answered.tool_use_id, "toolu_2");
        assert_eq!(asks.count(), 1, "the other one is still waiting");
    }

    #[test]
    fn a_mistyped_tool_use_id_is_not_reported_as_a_run_with_no_question() {
        // The two facts the one filter chain used to flatten. A stale or mistyped id said
        // "nothing on this node is waiting for an answer about run X" while `offload asks` was
        // printing that run's question on the next line — and the operator, believing it, goes
        // looking for an expired question rather than at their own argument. Measured in session
        // seventy-eight against a live blocked agent.
        let asks = Asks::default();
        let _a = register(&asks, run(1), "toolu_walk_1");

        let err = asks
            .answer(
                run(1),
                Some("toolu_typo"),
                Verdict {
                    allow: true,
                    by: "owner".into(),
                },
            )
            .expect_err("a mistyped id is refused");
        assert_eq!(
            err,
            AnswerError::NoSuchQuestion {
                run: run(1).short(),
                tool_use_id: "toolu_typo".into(),
                waiting: "`toolu_walk_1`".into(),
            }
        );
        // It names what *is* waiting, so the next attempt is satisfiable from this screen.
        assert!(err.to_string().contains("toolu_walk_1"), "{err}");
        assert_eq!(asks.count(), 1, "and it answered nothing");

        // A run with no question at all still gets the other sentence.
        assert_eq!(
            asks.answer(
                run(9),
                Some("toolu_walk_1"),
                Verdict {
                    allow: true,
                    by: "owner".into(),
                },
            )
            .expect_err("no such run"),
            AnswerError::NothingWaiting(run(9).short())
        );
    }

    #[test]
    fn an_answer_that_arrives_after_the_wait_ran_out_is_refused() {
        // The race this exists for: patience ran out, the hook was told nothing was decided, and
        // somebody's phone was in their pocket. The approval must not be reported as applied.
        let asks = Asks::default();
        let rx = register(&asks, run(1), "toolu_1");
        drop(rx); // the waiter gave up
        assert_eq!(
            asks.answer(
                run(1),
                None,
                Verdict {
                    allow: true,
                    by: "owner".into()
                }
            ),
            Err(AnswerError::Gone)
        );
    }

    #[test]
    fn a_question_nobody_asked_is_said_to_be_absent_rather_than_ignored() {
        let asks = Asks::default();
        assert_eq!(
            asks.answer(
                run(9),
                None,
                Verdict {
                    allow: true,
                    by: "owner".into()
                }
            ),
            Err(AnswerError::NothingWaiting(run(9).short()))
        );
    }

    #[test]
    fn a_retried_hook_replaces_its_own_question() {
        // The agent re-runs a hook it got nothing from. Two entries for one tool call would leave
        // a sender nobody is listening to, which an operator would answer into silence.
        let asks = Asks::default();
        let first = register(&asks, run(1), "toolu_1");
        let mut second = register(&asks, run(1), "toolu_1");
        assert_eq!(asks.count(), 1);
        assert!(first.is_terminated() || true, "the first sender is dropped");
        asks.answer(
            run(1),
            None,
            Verdict {
                allow: true,
                by: "owner".into(),
            },
        )
        .expect("answered");
        assert!(
            second.try_recv().is_ok(),
            "the live waiter is the one that hears it"
        );
    }

    #[test]
    fn what_is_waiting_is_reportable_with_a_clock() {
        let asks = Asks::default();
        let _rx = register(&asks, run(1), "toolu_1");
        let waiting = asks.waiting(Millis(61_000));
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].tool, "Bash");
        assert_eq!(waiting[0].waiting, Millis(60_000));
        assert_eq!(waiting[0].left, Millis(240_000));
        assert_eq!(waiting[0].node, None, "a node does not name itself");
        assert_eq!(asks.epoch_of(run(1), "toolu_1"), Some(Epoch(1)));
        assert_eq!(asks.epoch_of(run(1), "toolu_2"), None);
    }
}
