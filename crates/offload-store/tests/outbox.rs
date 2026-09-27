//! The promises the outbox makes, checked against outcome sequences nobody chose.
//!
//! ADR-0010's delivery plane is at-least-once with a dedup identity, and every claim in that
//! sentence is a claim about a table: that a notification noticed twice is owed once, that a
//! route which cannot be reached spends nothing, that a row is counted in exactly one of the
//! three numbers `offload sinks` prints, and that no route's backlog is another route's problem.
//! Each of those has already been got wrong once — two of them are in `CLAUDE.md` as rules
//! learned the hard way, and the third was found by writing this file.
//!
//! Driven against a real SQLite in memory rather than a model of one. The store's API is
//! synchronous by design (ADR-0009), so there is nothing to schedule and a generated sequence of
//! calls is just a loop.

// Panicking is how a property reports a counterexample. Same exemption as `offload-core`'s
// property tests, for the same reason.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use offload_core::notify::Topic;
use offload_core::{
    AgentKind, AgentWork, Constraint, Millis, NodeId, PermissionMode, Restartability, Run, RunId,
    RunSpec, ToolAllowlist, Work, WorkspaceSpec,
};
use offload_store::Store;
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

const RUN: RunId = RunId::from_bytes([3; 16]);

fn store() -> Store {
    let store = Store::open_memory().expect("open");
    let spec = RunSpec {
        work: Work::Agent(AgentWork {
            agent: AgentKind::ClaudeCode,
            model: None,
            prompt: "p".into(),
            workspace: WorkspaceSpec {
                repo: "/repo".into(),
                archive_bytes: None,
                git_ref: None,
                branch: None,
            },
            permission_mode: PermissionMode::AcceptEdits,
            allow: ToolAllowlist::default(),
            max_turns: None,
            ask: offload_core::AskPolicy::Never,
        }),
        constraint: Constraint::Always,
        restartability: Restartability::Resumable,
        priority: 0,
        queue: false,
        deadline: None,
        demand: offload_core::Demand::Normal,
        notify: offload_core::Audience::Everyone,
        notices: Default::default(),
        resources: Vec::new(),
        prefer: offload_core::Constraint::Always,
        hold_until: None,
        parent: None,
    };
    let run = Run::new(RUN, spec, NodeId::from_bytes([1; 32]), Millis(1));
    store.save_run(&run).expect("save");
    store
}

const SINKS: &[&str] = &["push", "email", "phone"];

/// What can happen to one outbox row, which is the whole of the delivery tick's vocabulary.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    Notice,
    Sent,
    Failed,
    /// The route could not be reached at all — "away is not gone".
    Deferred,
    Abandoned,
}

fn outcome() -> impl Strategy<Value = (u8, bool, Outcome, u8)> {
    (
        0u8..3,        // which sink
        any::<bool>(), // run topic or fleet topic
        prop_oneof![
            3 => Just(Outcome::Notice),
            2 => Just(Outcome::Sent),
            3 => Just(Outcome::Failed),
            3 => Just(Outcome::Deferred),
            1 => Just(Outcome::Abandoned),
        ],
        1u8..6, // which sequence number
    )
}

fn apply(store: &Store, sink: &str, topic: Topic, seq: u64, outcome: Outcome, now: Millis) {
    let run = matches!(topic, Topic::Run).then_some(RUN);
    match outcome {
        Outcome::Notice => {
            store
                .notice_delivery(sink, topic, seq, run, now)
                .expect("notice");
        }
        Outcome::Sent => store.delivery_sent(sink, topic, seq, now).expect("sent"),
        Outcome::Failed => {
            store.delivery_failed(sink, topic, seq, "no").expect("fail");
        }
        Outcome::Deferred => store
            .delivery_deferred(sink, topic, seq, "away")
            .expect("defer"),
        Outcome::Abandoned => store
            .delivery_abandoned(sink, topic, seq, now, "no")
            .expect("abandon"),
    }
}

proptest! {
    /// Every row is counted in exactly one of the three numbers `offload sinks` prints.
    ///
    /// The bug this stands against is in `CLAUDE.md`: delivered and given-up are *both* resolved
    /// rows, so a first version of `offload sinks` reported four successful deliveries for a
    /// route that had never worked once. Three numbers, and they have to add up — a row that
    /// falls out of all three is a delivery nobody can account for, and one that lands in two
    /// makes the totals lie in the reassuring direction.
    #[test]
    fn delivered_waiting_and_given_up_account_for_every_row(
        steps in prop::collection::vec(outcome(), 0..40),
    ) {
        let store = store();
        let mut keys: BTreeSet<(String, &str, u64)> = BTreeSet::new();
        for (i, (which, is_run, what, seq)) in steps.iter().enumerate() {
            let sink = SINKS[usize::from(*which)];
            let topic = if *is_run { Topic::Run } else { Topic::Fleet };
            let seq = u64::from(*seq);
            // Only a noticed row exists, and only an existing row can have an outcome — which
            // is what the delivery tick does, since it reads the pending rows to act on them.
            if matches!(what, Outcome::Notice) {
                keys.insert((sink.to_string(), topic.name(), seq));
            } else if !keys.contains(&(sink.to_string(), topic.name(), seq)) {
                continue;
            }
            apply(&store, sink, topic, seq, *what, Millis(1_000 + i as u64));
        }

        let mut counted = 0u64;
        for sink in SINKS {
            let totals = store.sink_totals(sink).expect("totals");
            counted += totals.delivered + totals.waiting + totals.gave_up;
        }
        prop_assert_eq!(
            counted,
            keys.len() as u64,
            "{} rows exist and {} were accounted for",
            keys.len(),
            counted
        );
    }

    /// A route that cannot be reached spends nothing from its retry budget.
    ///
    /// `CLAUDE.md`'s "a route that is away is not a route that is gone": a phone is asleep for
    /// hours, and a notification queued for it must wait rather than burn three attempts in
    /// forty-five seconds and be thrown away. So `delivery_deferred` records the reason and
    /// leaves the count alone — however many passes go by, and whatever else happened to the
    /// row first.
    #[test]
    fn being_away_is_free_however_long_it_lasts(
        before in prop::collection::vec(prop_oneof![Just(true), Just(false)], 0..4),
        defers in 1usize..12,
    ) {
        let store = store();
        store
            .notice_delivery("phone", Topic::Run, 1, Some(RUN), Millis(1))
            .expect("notice");
        // Some real attempts first, so this is not only about a fresh row.
        let mut expected = 0u32;
        for failed in &before {
            if *failed {
                store.delivery_failed("phone", Topic::Run, 1, "no").expect("fail");
                expected += 1;
            }
        }
        for _ in 0..defers {
            store
                .delivery_deferred("phone", Topic::Run, 1, "away")
                .expect("defer");
        }
        let pending = store.pending_deliveries(8).expect("pending");
        let row = pending
            .iter()
            .find(|p| p.sink == "phone")
            .expect("it is still owed");
        prop_assert_eq!(
            row.attempts,
            expected,
            "{} deferrals spent {} attempts",
            defers,
            row.attempts - expected
        );
    }

    /// No route's backlog is another route's problem.
    ///
    /// Found by writing this file, and it is the second-order failure of the rule above. A route
    /// that is away keeps its rows for ever *and* they are the oldest in the table, so a single
    /// page ordered by sequence was all phone: a device asleep for a weekend crowded every other
    /// route out of the queue entirely. Never attempted, so nothing was recorded — `offload
    /// sinks` showed a working desktop with a growing "waiting" count and no error beside it.
    #[test]
    fn a_route_with_work_to_do_is_always_offered_some(
        backlog in 1usize..60,
        budget in 1u32..12,
    ) {
        let store = store();
        // One route asleep, with a backlog of any size.
        for seq in 1..=backlog as u64 {
            store
                .notice_delivery("phone", Topic::Run, seq, Some(RUN), Millis(seq))
                .expect("notice");
            store
                .delivery_deferred("phone", Topic::Run, seq, "away")
                .expect("defer");
        }
        // …and two that are working, owed something that happened later.
        for sink in ["push", "email"] {
            store
                .notice_delivery(sink, Topic::Run, backlog as u64 + 1, Some(RUN), Millis(9_999))
                .expect("notice");
        }

        let page = store.pending_deliveries(budget).expect("pending");
        let mut by_sink: BTreeMap<&str, usize> = BTreeMap::new();
        for row in &page {
            *by_sink.entry(row.sink.as_str()).or_default() += 1;
        }
        for sink in ["push", "email", "phone"] {
            prop_assert!(
                by_sink.contains_key(sink),
                "{} has work owed and was offered none: {:?}",
                sink,
                by_sink
            );
        }
        // And a budget is a budget: nobody is offered more than it.
        for (sink, count) in &by_sink {
            prop_assert!(
                *count <= budget as usize,
                "{} was offered {} rows on a budget of {}",
                sink,
                count,
                budget
            );
        }
    }

    /// Noticing the same news twice owes it once, and forgets nothing about the first time.
    ///
    /// The at-least-once claim rests on this: a node that restarts mid-pass re-notices
    /// everything after its cursor, and the insert that loses the race has to be a no-op rather
    /// than a reset. If a re-notice cleared the attempt count or the reason, a route that had
    /// failed twice would start again from zero on every restart and never be given up on — and
    /// the reason somebody needs in order to fix it would be gone.
    #[test]
    fn re_noticing_is_a_no_op_rather_than_a_reset(
        failures in 1u32..5,
        renotices in 1usize..5,
    ) {
        let store = store();
        prop_assert!(store
            .notice_delivery("push", Topic::Run, 1, Some(RUN), Millis(1))
            .expect("notice"));
        for _ in 0..failures {
            store.delivery_failed("push", Topic::Run, 1, "the script exited 1").expect("fail");
        }
        for _ in 0..renotices {
            prop_assert!(
                !store
                    .notice_delivery("push", Topic::Run, 1, Some(RUN), Millis(2))
                    .expect("notice"),
                "a re-notice reported itself as new work"
            );
        }
        let pending = store.pending_deliveries(8).expect("pending");
        let row = pending.iter().find(|p| p.sink == "push").expect("owed");
        prop_assert_eq!(row.attempts, failures, "the attempt count was reset");
        prop_assert_eq!(
            row.last_error.as_deref(),
            Some("the script exited 1"),
            "the reason somebody needs was lost"
        );
    }

    /// A resolved row is never offered again, whichever way it resolved.
    ///
    /// Delivered and given-up are both resolved, and the queue must contain neither: sending a
    /// delivered notification again is the duplicate the dedup key exists to prevent, and
    /// retrying an abandoned one is the retry budget not meaning anything.
    #[test]
    fn a_resolved_row_is_never_offered_again(
        sent in prop::collection::vec(1u64..8, 0..6),
        abandoned in prop::collection::vec(1u64..8, 0..6),
    ) {
        let store = store();
        for seq in 1..=8u64 {
            store
                .notice_delivery("push", Topic::Run, seq, Some(RUN), Millis(seq))
                .expect("notice");
        }
        let mut resolved: BTreeSet<u64> = BTreeSet::new();
        for seq in &sent {
            store.delivery_sent("push", Topic::Run, *seq, Millis(100)).expect("sent");
            resolved.insert(*seq);
        }
        for seq in &abandoned {
            // Abandoning something already delivered is not something the tick does; skip it
            // rather than assert about a call nothing makes.
            if resolved.contains(seq) {
                continue;
            }
            store
                .delivery_abandoned("push", Topic::Run, *seq, Millis(200), "gone")
                .expect("abandon");
            resolved.insert(*seq);
        }

        let offered: BTreeSet<u64> = store
            .pending_deliveries(32)
            .expect("pending")
            .iter()
            .map(|p| p.seq)
            .collect();
        for seq in &resolved {
            prop_assert!(!offered.contains(seq), "seq {} was resolved and offered again", seq);
        }
        prop_assert_eq!(offered.len(), 8 - resolved.len(), "the rest are still owed");
    }
}
