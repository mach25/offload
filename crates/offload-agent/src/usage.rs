//! What a conversation has consumed, read from the agent's transcript (ADR-0040).
//!
//! **Why the transcript and not the event stream.** The streamed assistant `usage` is a partial
//! snapshot taken before the message is finished — measured on claude 2.1.251, three messages
//! reported `output_tokens` of 1, 2 and 4 where their finals were 236, 85 and 42, with
//! `stop_reason: null` on every streamed copy. The transcript carries the finals. Anything
//! costed or bounded from the stream is out by roughly fifty times, in the direction that looks
//! harmless.
//!
//! **Why deduplication is the whole of the parsing.** The agent writes one row per *content
//! block*, not per message: a thinking block and a tool call are two rows sharing one
//! `message.id` and one `usage` object. Summing rows double-counts exactly — 726 against a true
//! 363 for a two-block message. Deduplicated by `message.id`, the totals reconcile **exactly**
//! with the sum of every leg's `result` event on all four counters, verified across a fresh leg
//! and a `--resume`: input 44, output 517, cache-creation 9,474, cache-read 104,639.
//!
//! **These numbers are cumulative over the run's whole life**, because the transcript is — a
//! resumed leg appends to the same file. That is the opposite of `Outcome::cost_micro_usd`,
//! which is *per leg*, and the reason the two merge by opposite rules one crate up: a total from
//! here **replaces**, a cost from a result **adds**. Mixing them double-counts or erases.
//!
//! Deliberately no dollars. Converting tokens to money needs a price list, and ADR-0040 §1 is
//! why this crate will not ship one: the agent already reports `costBasis: "list"` beside its own
//! figure, and a second conversion here would be a third number disagreeing with two others,
//! stale the week a price changes and wrong with nothing to say so.

use offload_core::TokenUse;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Debug, Deserialize)]
struct Row {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Total the assistant messages in a transcript, counting each message once.
///
/// Lenient for `event.rs`'s reason and then some: this is somebody else's file format, read for
/// a *report* rather than for a decision, so an unparseable line is skipped rather than failing
/// the checkpoint it was read during. The cost of a wrong answer here is a number in a column;
/// the cost of an error is a run that cannot check point.
///
/// A row with no `message.id` is skipped rather than counted, because the id is what makes the
/// count correct — an unattributable row cannot be deduplicated, so including it is the
/// double-count this function exists to avoid.
#[must_use]
pub fn from_transcript(bytes: &[u8]) -> TokenUse {
    let mut seen = BTreeSet::new();
    let mut out = TokenUse::default();

    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_slice::<Row>(line) else {
            continue;
        };
        if row.r#type != "assistant" {
            continue;
        }
        let Some(message) = row.message else { continue };
        let (Some(id), Some(usage)) = (message.id, message.usage) else {
            continue;
        };
        // The deduplication, and the only thing in here that is not bookkeeping.
        if !seen.insert(id) {
            continue;
        }
        out.input = out.input.saturating_add(usage.input_tokens);
        out.output = out.output.saturating_add(usage.output_tokens);
        out.cache_creation = out
            .cache_creation
            .saturating_add(usage.cache_creation_input_tokens);
        out.cache_read = out.cache_read.saturating_add(usage.cache_read_input_tokens);
        out.messages = out.messages.saturating_add(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One assistant message, written the way the agent writes it: one row per content block,
    /// both carrying the same id and the same usage.
    fn message(id: &str, input: u64, output: u64, cc: u64, cr: u64) -> String {
        // Deliberately one line: `from_transcript` splits on newlines, so a fixture that wraps
        // is a fixture that tests nothing — which is how this test first "passed" at zero.
        let usage = format!(
            r#""usage":{{"input_tokens":{input},"output_tokens":{output},"cache_creation_input_tokens":{cc},"cache_read_input_tokens":{cr}}}"#
        );
        format!(
            "{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"stop_reason\":\"tool_use\",\
             \"content\":[{{\"type\":\"thinking\"}}],{usage}}}}}\n\
             {{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"stop_reason\":\"tool_use\",\
             \"content\":[{{\"type\":\"tool_use\"}}],{usage}}}}}\n"
        )
    }

    #[test]
    fn a_message_split_across_content_blocks_is_counted_once() {
        // The measurement this module exists for. Summing rows gives exactly double for a
        // two-block message — 726 against a true 363 on the real run this was found on — and the
        // error is invisible because both numbers look plausible.
        let transcript = message("msg_a", 10, 236, 7260, 13979);
        let used = from_transcript(transcript.as_bytes());
        assert_eq!(used.messages, 1, "two rows, one message");
        assert_eq!(used.output, 236, "and not 472");
        assert_eq!(used.input, 10);
        assert_eq!(used.cache_creation, 7260);
        assert_eq!(used.cache_read, 13979);
    }

    #[test]
    fn the_totals_reconcile_with_what_the_agent_reported_for_the_whole_conversation() {
        // The three messages of a real fresh leg, then the two of its `--resume`, in one
        // transcript — because the transcript accumulates across legs and that is exactly the
        // property one crate up depends on. Measured against claude 2.1.251; the two `result`
        // events reported input 26/18, output 363/154, cache-read 58,237/46,402.
        let mut transcript = String::new();
        transcript.push_str(&message("msg_1", 10, 236, 7260, 13979));
        transcript.push_str(&message("msg_2", 8, 85, 1780, 21239));
        transcript.push_str(&message("msg_3", 8, 42, 135, 23019));
        transcript.push_str(&message("msg_4", 10, 112, 264, 23300));
        transcript.push_str(&message("msg_5", 8, 42, 35, 23102));

        let used = from_transcript(transcript.as_bytes());
        assert_eq!(
            used.messages, 5,
            "three from the first leg, two from the resume"
        );
        assert_eq!(used.input, 44, "26 + 18");
        assert_eq!(used.output, 517, "363 + 154");
        assert_eq!(used.cache_read, 104_639, "58,237 + 46,402");
        assert_eq!(used.cache_creation, 9_474);
    }

    #[test]
    fn nothing_in_a_transcript_it_cannot_read_is_counted_or_fatal() {
        // Read during a checkpoint, for a report. An unparseable line must cost a number in a
        // column and never the capture it was read during.
        let mut transcript = String::from("not json at all\n\n{\"type\":\"user\"}\n");
        transcript
            .push_str("{\"type\":\"assistant\",\"message\":{\"usage\":{\"output_tokens\":99}}}\n");
        transcript.push_str("{\"type\":\"assistant\",\"message\":{\"id\":\"x\"}}\n");
        transcript.push_str(&message("msg_real", 1, 2, 3, 4));

        let used = from_transcript(transcript.as_bytes());
        assert_eq!(used.messages, 1, "only the well-formed assistant message");
        assert_eq!(
            used.output, 2,
            "and the row with usage but no id is skipped rather than counted — it cannot be \
             deduplicated, so counting it is the double-count this exists to avoid"
        );

        // And an empty transcript says so, rather than reporting a conversation of zero cost.
        assert!(from_transcript(b"").is_empty());
        assert!(!used.is_empty());
    }
}
