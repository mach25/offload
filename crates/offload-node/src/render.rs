//! What a run's log says, as lines of text — for the CLI's `offload logs` and the app's run view
//! alike (ADR-0071 §2), so a terminal and a phone cannot word one event two ways.

use offload_core::LogKind;

/// Is a run in this state over — what `offload ps` hides without `--all`, and the app's "active"
/// filter hides too. One rule, so the two lists agree about which runs are still going.
#[must_use]
pub fn is_finished(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "cancelled")
}

/// How long until an absolute unix-millisecond instant, from `now_ms`, for saying it back.
#[must_use]
pub fn until(deadline_ms: u64, now_ms: u64) -> String {
    offload_core::Millis(deadline_ms.saturating_sub(now_ms)).to_string()
}

/// The agent line of a run's log: what is known of the agent and model, and the absence of what
/// is not.
#[must_use]
pub fn agent_line(model: &str, version: &str) -> String {
    let agent = if version.is_empty() {
        "claude-code (version not reported)".to_string()
    } else {
        format!("claude-code {version}")
    };
    let model = if model.is_empty() {
        "model not reported".to_string()
    } else {
        format!("model {model}")
    };
    format!("{agent}, {model}")
}

/// `run` is what the operator typed, not the resolved id, and that is deliberate: it is the
/// one spelling known to resolve on this machine, and the only line here that is meant to be
/// pasted needs a spelling rather than a placeholder.
pub fn event_lines(kind: &LogKind, run: &str, now_ms: u64) -> Vec<String> {
    let mut out = Vec::new();
    match kind {
        LogKind::Submitted {
            repo,
            prompt,
            permission,
        } => {
            // Said, not spelled: `scratch:` is the spec's name for "no repository" (ADR-0072).
            let repo = if repo == offload_core::SCRATCH {
                "an empty workspace"
            } else {
                repo.as_str()
            };
            out.push(format!("submitted   {repo} [{permission}]"));
            out.push(format!("prompt      {prompt}"));
        }
        LogKind::WorkspaceAcquired { archive, summary } => {
            // Above `workspace`, in the order it happened: the bytes arrive, then the worktree
            // is built from them. What it says is the answer to "was that file supposed to be
            // here" — the one question a subset workspace makes possible and a clone does not.
            out.push(format!("archive     {archive}"));
            out.push(format!("            {summary}"));
        }
        LogKind::WorkspaceReady { path, branch, base } => {
            out.push(format!("workspace   {path}"));
            out.push(format!("branch      {branch} from {base}"));
        }
        LogKind::Continued {
            parent,
            mode,
            base,
            transcript_bytes,
            missing,
        } => {
            // The parent **whole**: it is what somebody types next, into `offload logs` or
            // `offload continue`, and a prefix on this line is an instruction that may not resolve.
            out.push(format!("continues   {parent} ({mode})"));
            out.push(format!("from        {base}"));
            if let Some(bytes) = transcript_bytes {
                out.push(format!(
                    "transcript  {bytes} bytes at {}, for the agent to read if it needs to",
                    offload_core::PARENT_TRANSCRIPT
                ));
            }
            for gap in missing {
                out.push(format!("note        {gap}"));
            }
        }
        LogKind::AgentStarted { model, version, .. } => {
            // Built from what is actually known. Both fields come from the agent's own init
            // line, which is somebody else's format read leniently (`event.rs`), so either can
            // be absent — and an absent one used to render as a **gap**: `agent claude-code ,
            // model`, two empty slots where the facts should be. That reads as a broken report
            // rather than as a missing fact, and sends somebody to look at the wrong thing.
            //
            // It is the sentence `TaskStarted` was split into its own variant to avoid, reached
            // from the agent side instead of the task side — and the same rule: don't over-claim
            // in a log line. Naming the absence is not decoration here, because *which model did
            // this run spend on* is a question somebody asks of this exact line.
            out.push(format!("agent       {}", agent_line(model, version)));
            out.push(String::new());
        }
        LogKind::TaskStarted { service } => {
            // The service and nothing else, because the service is the whole of what the
            // submitter named and the command is the holder's business (ADR-0019 §1). Somebody
            // reading this log on another machine has no business learning what ran there.
            out.push(format!("task        {service}"));
            out.push(String::new());
        }
        LogKind::Asked {
            tool,
            detail,
            tool_use_id,
            within_ms,
        } => {
            // Mid-stream and impossible to miss: the run is *stopped* here until somebody
            // answers, which is not true of any other line in this log.
            out.push(format!("?? waiting for you: may it use {tool}? {detail}"));
            // The run, spelled, rather than `<run>`: an instruction that cannot be pasted is
            // not one, and `offload asks` one command away printed the id. **There are two such
            // lines in this log, not one** — `Checkpointed`'s release note is the other, and it
            // kept its placeholder for four sessions because this comment said there was only
            // this one and nobody went looking.
            out.push(format!(
                "   offload approve {run} {tool_use_id}   (or `offload deny`) — within {}",
                offload_core::Millis(*within_ms)
            ));
        }
        LogKind::Answered {
            answer,
            by,
            tool_use_id: _,
            tool: _,
        } => out.push(format!("   -> {answer} ({by})")),
        // Not an error, and phrased so it does not read as one: nothing failed, the run simply
        // stops interrupting anybody and behaves the way a run without --ask always has.
        LogKind::AskBudgetSpent { asked } => out.push(format!(
            "-- that was question {asked} of {asked}; the rest is up to the agent's own rules"
        )),
        LogKind::Text { text } => out.push(text.trim_end().to_string()),
        LogKind::ToolUse { name } => out.push(format!("  · {name}")),
        LogKind::TurnBoundary { turn } => out.push(format!("── turn {turn} ──")),
        LogKind::Checkpointed {
            turn,
            summary,
            released,
        } => {
            out.push(format!("checkpoint  turn {turn}: {summary}"));
            if *released {
                // Spelled, for the reason the `Asked` arm below gives — and this is the line
                // that shows the reason was stated one case too narrowly. That comment says
                // *"the one line in the log that is an instruction"*, which was not true when
                // it was written: this is the other one, in the same `match`, forty lines up,
                // and it kept its `<run>` through the fix that removed the sibling's.
                out.push(format!(
                    "run released — resume it with: offload resume {run}"
                ));
            }
        }
        LogKind::Resumed {
            from_turn,
            session,
            workspace,
        } => {
            out.push(format!(
                "resumed     from turn {from_turn}, session {session}"
            ));
            out.push(format!("workspace   {workspace}"));
        }
        LogKind::RateLimit {
            kind,
            status,
            resets_at_unix_ms,
        } => {
            if status != "allowed" {
                match resets_at_unix_ms {
                    Some(at) => out.push(format!(
                        "!! rate limit ({kind}): {status}, lifts in {}",
                        until(*at, now_ms)
                    )),
                    None => out.push(format!("!! rate limit ({kind}): {status}")),
                }
            }
        }
        // Deliberately loud, and deliberately not the end of the run: a deadline decides when
        // we give up, never what happens to the work (ADR-0013). The run is still going.
        LogKind::Overdue { by_ms, waiting } => {
            out.push(format!(
                "!! this run will not make its deadline — {} past it, and {waiting}",
                offload_core::Millis(*by_ms)
            ));
        }
        LogKind::Finished {
            success,
            turns,
            denials,
            cost_micro_usd,
            work,
            result,
        } => {
            out.push(String::new());
            // A task's closing line is the fact and nothing else: turns and dollars are an
            // agent's units, and this printed `finished after 0 turn(s), $0.0000` under a
            // shell script's own output. Its failure is a `Failed { reason }` carrying the
            // exit status, which is where the rest of a task's answer already was.
            match work {
                offload_core::WorkKind::Task if *success => out.push("finished".to_string()),
                offload_core::WorkKind::Task => out.push("FAILED".to_string()),
                offload_core::WorkKind::Agent => out.push(format!(
                    "{} after {turns} turn(s), ${:.4}",
                    if *success { "finished" } else { "FAILED" },
                    *cost_micro_usd as f64 / 1_000_000.0
                )),
            }
            if *denials > 0 {
                out.push(format!(
                    "{denials} permission request(s) denied — the agent was blocked from \
                     something it wanted to do.\nRe-run with --permission full if that was \
                     the intent (see docs/adr/0008-headless-permissions.md)."
                ));
            }
            // The agent's answer, which is what somebody asked for. Kept on the record since
            // ADR-0064 and shown nowhere: a run asked to summarise three days of email finished
            // with "finished after 1 turn(s)" and the summary only in the database (session
            // ninety-two). Verbatim, one entry per line, under a heading of its own.
            if let Some(answer) = result.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                out.push(String::new());
                out.push("answer".to_string());
                out.extend(answer.lines().map(str::to_string));
            }
        }
        LogKind::Cancelled { by } => out.push(format!("\ncancelled from {by}")),
        LogKind::Failed { reason } => out.push(format!("\nfailed: {reason}")),
        // Mid-stream, and not preceded by a blank line: the run is still going, which is the
        // whole distinction this variant exists to make — when it is still going. A capture
        // that failed at the boundary a turn limit stops on has no later boundary to try at,
        // and the reassurance is then the opposite of the truth.
        LogKind::CaptureFailed {
            turn,
            reason,
            run_continues,
        } => {
            out.push(format!("!! checkpoint at turn {turn} failed: {reason}"));
            if *run_continues {
                out.push("   the run continues; the next turn boundary tries again".to_string());
            } else {
                // Said in the run's own log, where somebody looking for the conversation will
                // be. It is the last thing this run had to say about the work: there is no
                // checkpoint from this turn and nothing later will make one.
                out.push(
                    "   this was the run's last turn, so nothing tries again — it has no"
                        .to_string(),
                );
                out.push(format!(
                    "   checkpoint from turn {turn} and no way back into the conversation"
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer is what somebody asked for, and it was on the record and printed nowhere.
    #[test]
    fn a_finished_run_shows_the_agents_answer() {
        let finished = |result: Option<&str>| LogKind::Finished {
            turns: 2,
            cost_micro_usd: 1_000,
            denials: 0,
            success: true,
            work: offload_core::WorkKind::Agent,
            result: result.map(str::to_string),
        };
        let lines = event_lines(
            &finished(Some("Three emails:\n- the invoice\n- the meeting")),
            "r",
            0,
        );
        let at = lines
            .iter()
            .position(|l| l == "answer")
            .expect("an answer heading");
        assert_eq!(
            &lines[at + 1..],
            ["Three emails:", "- the invoice", "- the meeting"]
        );
        let quiet = event_lines(&finished(None), "r", 0);
        assert!(
            !quiet.iter().any(|l| l == "answer"),
            "no heading over nothing: {quiet:?}"
        );
        let blank = event_lines(&finished(Some("  \n")), "r", 0);
        assert!(
            !blank.iter().any(|l| l == "answer"),
            "nor over whitespace: {blank:?}"
        );
    }
}
