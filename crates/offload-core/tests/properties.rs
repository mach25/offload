//! The invariants this crate exists to keep, checked against sequences nobody chose.
//!
//! Every unit test in `offload-core` arranges a case somebody thought of. That is the right way
//! to pin down a decision, and it is exactly the wrong way to find out what happens when a
//! reclaim arrives after a release, at a stale epoch, from the node that used to be the holder
//! — the shape of thing a fleet produces all night and nobody writes down. So: generate the
//! sequence, and assert the properties that must hold whatever it was.
//!
//! This is the phase-0 roadmap item (`property tests: no run lost across transitions, epochs
//! monotonic under churn`) and it is what ADR-0001's whole arrangement was for. A crate with a
//! clock in it cannot be driven like this: `now` would arrive from the machine instead of from
//! the test, so "the same sequence 200 milliseconds later" would not be a case anybody could
//! ask for.
//!
//! **What a failure here means.** These are not regression tests for bugs that happened; they
//! are the sentences the rest of the tree is entitled to assume. A counterexample is either a
//! real defect or a rule nobody had written down — and the second is worth as much as the
//! first, because the code below is the only place some of these are stated.

// proptest reports a counterexample by panicking, which is the whole mechanism, and
// `prop_assert!` is how a property says what it wanted. Same exemption as `no_clock.rs`, for
// the same reason: an integration test is linted as ordinary code, which is right nearly
// everywhere and not where panicking *is* the reporting.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use offload_core::bid::{Availability, Bid, Score};
use offload_core::capability::AgentKind;
use offload_core::capacity::Demand;
use offload_core::constraint::Constraint;
use offload_core::fleet::{Delegation, FleetId, Grant, Issuer, MembershipCert, Terms};
use offload_core::id::{BlobHash, NodeId, RunId};
use offload_core::progress::RunProgress;
use offload_core::run::{
    Checkpoint, Epoch, GivenUp, PermissionMode, Restartability, Run, RunSpec, RunState, SpecEdit,
    WorkspaceSpec, LEASE,
};
use offload_core::time::Millis;
use offload_core::view::{ClusterView, NodeView};
use offload_core::AskPolicy;
use offload_core::{AgentWork, Work};
use offload_core::{Risk, ToolAllowlist, ToolPattern};
use proptest::prelude::*;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Fixtures. Deliberately minimal: these properties are about the machinery, and a spec that
// varied would only widen the search space with fields nothing here reads.
// ---------------------------------------------------------------------------

fn node(b: u8) -> NodeId {
    NodeId::from_bytes([b; 32])
}

fn spec(restartability: Restartability) -> RunSpec {
    RunSpec {
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
            allow: Default::default(),
            max_turns: None,
            ask: AskPolicy::Never,
        }),
        constraint: Constraint::Always,
        restartability,
        priority: 0,
        queue: false,
        deadline: None,
        demand: Demand::Normal,
        notify: Default::default(),
        notices: Default::default(),
        resources: Vec::new(),
        prefer: offload_core::Constraint::Always,
        hold_until: None,
        parent: None,
    }
}

fn checkpoint(at: Millis) -> Checkpoint {
    Checkpoint {
        session_id: Some("s".into()),
        transcript: BlobHash::from_bytes([1; 32]),
        bundle: Some(BlobHash::from_bytes([2; 32])),
        patch: None,
        base_commit: "abc".into(),
        turns: 1,
        taken_at: at,
        agent_version: "2.1.0".into(),
        replicas: BTreeSet::new(),
    }
}

// ---------------------------------------------------------------------------
// The run state machine, driven by sequences nobody chose
// ---------------------------------------------------------------------------

/// Which epoch a caller presents, relative to the run's current one.
///
/// Generated as an offset rather than as a number, because the interesting cases are all
/// *relative*: a stale holder presents `Behind`, a healthy one `Current`, and `Ahead` is the
/// one nothing legitimate can produce — which is the reason to generate it. `fence` compares
/// against the run's epoch, so nothing about a raw `u64` would find the boundary on its own.
#[derive(Debug, Clone, Copy)]
enum Presented {
    Current,
    Behind(u64),
    Ahead(u64),
}

impl Presented {
    fn resolve(self, current: Epoch) -> Epoch {
        match self {
            Presented::Current => current,
            Presented::Behind(n) => Epoch(current.0.saturating_sub(n.max(1))),
            Presented::Ahead(n) => Epoch(current.0 + n.max(1)),
        }
    }
}

/// One thing that can be done to a run. Everything `Run` exposes that mutates.
#[derive(Debug, Clone)]
enum Act {
    Assign(u8),
    Started(u8, Presented),
    Renew(u8, Presented),
    Orphan,
    Reclaim(u8, Presented),
    RequestCheckpoint,
    /// The `bool` is [`GivenUp`]: `true` for a node letting the run go, `false` for a
    /// person parking it. Generated rather than fixed, because the two write different
    /// `Pending` states and every invariant below has to hold for both.
    Checkpointed(u8, Presented, bool),
    Release(u8, Presented),
    RecordCheckpoint(u8, Presented),
    Complete(u8, Presented),
    Fail(u8, Presented),
    Cancel,
    Abandon,
    Reopen,
    Edit(i32),
}

fn presented() -> impl Strategy<Value = Presented> {
    prop_oneof![
        // Weighted towards the legitimate case, so a sequence gets somewhere before it starts
        // being refused: a walk that is stale at every step never reaches `Running` at all.
        6 => Just(Presented::Current),
        2 => (1u64..3).prop_map(Presented::Behind),
        1 => (1u64..3).prop_map(Presented::Ahead),
    ]
}

fn act() -> impl Strategy<Value = Act> {
    // Three nodes is the smallest fleet where "the holder", "the node that used to hold it"
    // and "somebody else entirely" are three different answers.
    let who = 1u8..4;
    prop_oneof![
        4 => who.clone().prop_map(Act::Assign),
        4 => (who.clone(), presented()).prop_map(|(n, e)| Act::Started(n, e)),
        2 => (who.clone(), presented()).prop_map(|(n, e)| Act::Renew(n, e)),
        3 => Just(Act::Orphan),
        3 => (who.clone(), presented()).prop_map(|(n, e)| Act::Reclaim(n, e)),
        2 => Just(Act::RequestCheckpoint),
        2 => (who.clone(), presented(), any::<bool>())
            .prop_map(|(n, e, l)| Act::Checkpointed(n, e, l)),
        2 => (who.clone(), presented()).prop_map(|(n, e)| Act::Release(n, e)),
        2 => (who.clone(), presented()).prop_map(|(n, e)| Act::RecordCheckpoint(n, e)),
        1 => (who.clone(), presented()).prop_map(|(n, e)| Act::Complete(n, e)),
        1 => (who.clone(), presented()).prop_map(|(n, e)| Act::Fail(n, e)),
        1 => Just(Act::Cancel),
        1 => Just(Act::Abandon),
        2 => Just(Act::Reopen),
        1 => (-5i32..5).prop_map(Act::Edit),
    ]
}

/// Apply one act, and say whether the run accepted it.
fn apply(run: &mut Run, act: &Act, now: Millis) -> bool {
    let e = run.epoch;
    match act {
        Act::Assign(n) => run.assign(node(*n), now, LEASE).is_ok(),
        Act::Started(n, p) => run.started(node(*n), p.resolve(e), now).is_ok(),
        Act::Renew(n, p) => run.renew(node(*n), p.resolve(e), now, LEASE).is_ok(),
        Act::Orphan => run.orphan(now).is_ok(),
        Act::Reclaim(n, p) => run.reclaim(node(*n), p.resolve(e), now, LEASE).is_ok(),
        Act::RequestCheckpoint => run.request_checkpoint(now).is_ok(),
        Act::Checkpointed(n, p, let_go) => run
            .checkpointed(
                node(*n),
                p.resolve(e),
                checkpoint(now),
                if *let_go {
                    GivenUp::LetGo
                } else {
                    GivenUp::Parked
                },
                now,
            )
            .is_ok(),
        Act::Release(n, p) => run.release(node(*n), p.resolve(e), now).is_ok(),
        Act::RecordCheckpoint(n, p) => run
            .record_checkpoint(node(*n), p.resolve(e), checkpoint(now))
            .is_ok(),
        Act::Complete(n, p) => run.complete(node(*n), p.resolve(e), now).is_ok(),
        Act::Fail(n, p) => run.fail(node(*n), p.resolve(e), "because", now).is_ok(),
        Act::Cancel => run.cancel(now).is_ok(),
        Act::Abandon => run.abandon("because", now).is_ok(),
        Act::Reopen => run.reopen(now).is_ok(),
        Act::Edit(p) => {
            run.edit(SpecEdit::Priority { to: *p });
            true
        }
    }
}

/// Everything that must be true of a run at rest, whatever was done to it.
fn check(run: &Run) -> Result<(), TestCaseError> {
    match &run.state {
        // The fencing invariant, and the quiet one: `fence` compares a caller's epoch against
        // `run.epoch` and then checks the *lease's* node. Those are two fields, and they are
        // only the same question while they agree. If a lease could outlive the epoch that
        // issued it, a holder from a previous assignment would pass the epoch test on the
        // strength of a lease nobody had reissued — which is the double-execution this whole
        // mechanism exists to prevent.
        RunState::Assigned { lease }
        | RunState::Running { lease, .. }
        | RunState::Checkpointing { lease, .. } => {
            prop_assert_eq!(
                lease.epoch,
                run.epoch,
                "a live lease at an epoch the run has left behind"
            );
            prop_assert_eq!(run.holder(), Some(lease.node));
        }
        // ADR-0007, and stated in CLAUDE.md as a rule of its own: the last holder has a right
        // to *reclaim*, and no right to act. A live lease here would hand an out-of-contact
        // node authority over a run that is being moved.
        RunState::Orphaned { last, .. } => {
            prop_assert!(run.state.lease().is_none(), "orphaned returned a lease");
            prop_assert_eq!(run.holder(), Some(last.node));
            prop_assert_eq!(last.epoch, run.epoch, "orphaned from a stale lease");
        }
        RunState::Pending { .. } => {
            prop_assert!(run.holder().is_none(), "pending with a holder");
        }
        state if state.is_terminal() => {
            prop_assert!(run.state.finished_at().is_some(), "terminal with no time");
            prop_assert!(run.holder().is_none(), "finished with a holder");
        }
        _ => {}
    }

    // `fence` is the guard every side-effecting transition goes through, so what it admits is
    // the definition of who may act. Restated here from the outside: exactly the current
    // holder, at not-behind the current epoch, in a state that has a lease.
    for n in 1u8..4 {
        for p in [
            Presented::Behind(1),
            Presented::Current,
            Presented::Ahead(1),
        ] {
            let admitted = run.fence(node(n), p.resolve(run.epoch)).is_ok();
            let should = !run.state.is_terminal()
                && run.state.lease().is_some_and(|l| l.node == node(n))
                && !matches!(p, Presented::Behind(_));
            prop_assert_eq!(admitted, should, "fence admitted the wrong caller");
        }
    }
    Ok(())
}

proptest! {
    /// Epoch monotonicity, and the two other counters that only go one way.
    ///
    /// The epoch is the fencing token: everything that makes it safe to reassign a run whose
    /// holder we merely *believe* is gone rests on it never going backwards. `attempts` drives
    /// the backoff that stops a run ping-ponging across the fleet, and `spec_rev` is what makes
    /// an operator's edit win a merge — both are useless the moment they can decrease.
    #[test]
    fn nothing_that_only_grows_ever_shrinks(
        acts in prop::collection::vec(act(), 0..24),
        pinned in any::<bool>(),
    ) {
        let mut run = Run::new(
            RunId::from_bytes([7; 16]),
            spec(if pinned { Restartability::Pinned } else { Restartability::Resumable }),
            node(9),
            Millis(0),
        );
        let mut now = Millis(0);
        check(&run)?;
        for act in &acts {
            let before = run.clone();
            now = now + Millis(1_000);
            apply(&mut run, act, now);
            prop_assert!(run.epoch >= before.epoch, "epoch went backwards: {:?}", act);
            prop_assert!(run.attempts >= before.attempts, "attempts went backwards");
            prop_assert!(run.spec_rev >= before.spec_rev, "spec_rev went backwards");
            check(&run)?;
        }
    }

    /// A refused transition changes nothing at all.
    ///
    /// The state machine's error path is where a partial mutation would hide: a function that
    /// writes a field and *then* discovers the caller is stale has already happened. Nothing
    /// downstream would notice — the call returned `Err`, so the caller reports a refusal and
    /// moves on, while the run quietly carries a change nobody authorised.
    ///
    /// This is the in-the-small version of what CLAUDE.md says about `drive`: a guard and the
    /// thing it guards must not be separated by anything that can take effect.
    #[test]
    fn a_refusal_is_not_a_partial_write(
        acts in prop::collection::vec(act(), 0..24),
    ) {
        let mut run = Run::new(
            RunId::from_bytes([7; 16]),
            spec(Restartability::Resumable),
            node(9),
            Millis(0),
        );
        let mut now = Millis(0);
        for act in &acts {
            let before = run.clone();
            now = now + Millis(1_000);
            if !apply(&mut run, act, now) {
                prop_assert_eq!(&run, &before, "a refused {:?} left a mark", act);
            }
        }
    }

    /// A run that has not finished has a way to finish, without an operator.
    ///
    /// The roadmap's "no run lost", stated so it can fail: a run in a non-terminal state must
    /// be reachable by *something the fleet does by itself*. `cancel` and `abandon` do not
    /// count — those are a person intervening, and a run that needs one is precisely a lost
    /// run. So the question is whether some node could still take it: the holder can reclaim
    /// it, a fresh node can be assigned it, or the run is held right now and its holder will
    /// report an end.
    #[test]
    fn a_run_that_has_not_finished_can_still_be_taken_by_somebody(
        acts in prop::collection::vec(act(), 0..24),
        pinned in any::<bool>(),
    ) {
        let restartability = if pinned { Restartability::Pinned } else { Restartability::Resumable };
        let mut run = Run::new(
            RunId::from_bytes([7; 16]),
            spec(restartability),
            node(9),
            Millis(0),
        );
        let mut now = Millis(0);
        for act in &acts {
            now = now + Millis(1_000);
            apply(&mut run, act, now);
        }
        if run.state.is_terminal() {
            return Ok(());
        }
        let later = now + Millis(60_000);
        // Held by somebody: whoever holds it can end it, and if they vanish the hold-down
        // reaches one of the cases below.
        if run.state.lease().is_some() {
            return Ok(());
        }
        let mut escapes = Vec::new();
        if run.clone().assign(node(4), later, LEASE).is_ok() {
            escapes.push("assignable");
        }
        if let Some(holder) = run.holder() {
            if run.clone().reclaim(holder, run.epoch, later, LEASE).is_ok() {
                escapes.push("reclaimable");
            }
            // A pinned run whose holder is gone is abandoned by the hold-down
            // (`decide_reassignment`), which is a decision and not an accident.
            if matches!(run.state, RunState::Orphaned { .. })
                && run.spec.restartability == Restartability::Pinned
            {
                escapes.push("abandoned by the hold-down");
            }
        }
        prop_assert!(
            !escapes.is_empty(),
            "nothing can happen to this run any more: {:?} attempts={} restartability={:?}",
            run.state,
            run.attempts,
            run.spec.restartability,
        );
    }
}

// ---------------------------------------------------------------------------
// The merge, which is where two nodes either agree or quietly do not
// ---------------------------------------------------------------------------

/// A gossiped record, and who this node heard it from.
///
/// The `from` is half of `merge_run`'s rule and the half a rank comparison would leave out:
/// `Orphaned` is not a lesser state than `Running`, it is the arbiter's observation, and a
/// third node relaying its own stale copy must not undo it.
#[derive(Debug, Clone)]
struct Heard {
    run: Run,
    from: NodeId,
}

/// Records for one run, as a fleet would produce them: the same run at assorted epochs and
/// states, each attributed to a node that might or might not be entitled to say it.
fn heard(home: NodeId) -> impl Strategy<Value = Heard> {
    (
        0u64..4,                  // epoch
        0u8..8,                   // state
        1u8..4,                   // holder
        1u8..5,                   // from
        0u32..3,                  // spec_rev
        prop::option::of(0u8..2), // checkpoint
    )
        .prop_map(move |(epoch, state, holder, from, spec_rev, cp)| {
            let mut run = Run::new(
                RunId::from_bytes([7; 16]),
                spec(Restartability::Resumable),
                home,
                Millis(0),
            );
            run.epoch = Epoch(epoch);
            run.spec_rev = spec_rev;
            run.spec.priority = i32::try_from(spec_rev).unwrap_or(0);
            run.checkpoint = cp.map(|_| checkpoint(Millis(1)));
            let lease = offload_core::run::Lease {
                node: node(holder),
                epoch: Epoch(epoch),
                expires_at: Millis(60_000),
            };
            run.state = match state {
                0 => RunState::Pending {
                    since: Millis(1),
                    let_go_by: None,
                },
                1 => RunState::Assigned { lease },
                2 => RunState::Running {
                    lease,
                    started_at: Millis(1),
                },
                3 => RunState::Checkpointing {
                    lease,
                    requested_at: Millis(1),
                },
                4 => RunState::Orphaned {
                    last: lease,
                    since: Millis(1),
                },
                5 => RunState::Completed { at: Millis(2) },
                6 => RunState::Failed {
                    at: Millis(2),
                    reason: "r".into(),
                },
                _ => RunState::Cancelled { at: Millis(2) },
            };
            Heard {
                run,
                from: node(from),
            }
        })
}

fn view_of(local: NodeId, members: &[u8], now: Millis) -> ClusterView {
    let mut view = ClusterView::new(local);
    for b in members {
        view.upsert_node(NodeView::new(
            node(*b),
            offload_core::capability::Capabilities::empty(
                offload_core::capability::Os::Linux,
                offload_core::capability::Arch::X86_64,
                offload_core::capability::DeviceClass::Desktop,
            ),
            offload_core::policy::WorkPolicy::for_class(
                offload_core::capability::DeviceClass::Desktop,
            ),
            now,
        ));
    }
    view
}

proptest! {
    /// A merge never walks a run's epoch back, and never reopens one that finished.
    ///
    /// These are the two things every reader of the view is entitled to assume. The epoch is
    /// the fencing token, so a merge that lowered it would hand a node that had been fenced out
    /// its authority back; and a terminal state that a gossiped record could undo would have
    /// `offload ps` reporting a finished run as running, on whichever node last heard from the
    /// stalest peer.
    #[test]
    fn merging_gossip_never_walks_a_run_backwards(
        records in prop::collection::vec(heard(node(9)), 1..8),
    ) {
        let mut view = view_of(node(1), &[1, 2, 3, 9], Millis(0));
        let mut high = Epoch::INITIAL;
        let mut terminal_at: Option<Epoch> = None;
        for h in &records {
            view.merge_run(h.run.clone(), h.from);
            let Some(now) = view.runs.get(&h.run.id) else { continue };
            prop_assert!(now.epoch >= high, "the view's epoch went backwards");
            high = now.epoch;
            // Finished is finished *at that epoch*. A higher epoch is a later decision and is
            // allowed to say the run is being tried again — that is reassignment after a
            // holder was concluded dead, and it is the one case where a completion can be
            // superseded rather than ignored.
            if let Some(at) = terminal_at {
                if now.epoch == at {
                    prop_assert!(now.state.is_terminal(), "a finished run was reopened at its own epoch");
                }
            }
            terminal_at = now.state.is_terminal().then_some(now.epoch);
        }
    }

    /// Only the holder may refute an orphan, and only the arbiter may declare one.
    ///
    /// The bug this stands against keeps a run pinned to a machine that no longer exists, for
    /// as long as anybody remembers it working: a third node relays its own stale `Running`
    /// and the arbiter's decision evaporates. Stated as a property because the entitlement is
    /// what matters, not the two states — any pair of records where the *speaker* is neither
    /// the holder nor the arbiter must leave an orphan alone.
    #[test]
    fn a_relayed_record_cannot_undo_an_arbiter(
        // Node 2 is watching, rather than holding, so it is excluded here rather than rejected
        // afterwards. A node holding the run is the one party that does *not* take the arbiter's
        // word about it — its own presence is the thing being claimed against, and that is
        // `merge_node`'s rule about our own liveness arriving at the run record. Its own
        // property, in `view.rs`; this one is about bystanders.
        holder in prop_oneof![Just(1u8), Just(3u8)],
        relay in 1u8..5,
        epoch in 1u64..3,
    ) {
        // Node 1 arbitrates: it is the run's home and it is alive.
        let arbiter = node(1);
        let mut view = view_of(node(2), &[1, 2, 3, 4, 9], Millis(0));
        let mut run = Run::new(
            RunId::from_bytes([7; 16]),
            spec(Restartability::Resumable),
            arbiter,
            Millis(0),
        );
        run.epoch = Epoch(epoch);
        let lease = offload_core::run::Lease {
            node: node(holder),
            epoch: Epoch(epoch),
            expires_at: Millis(60_000),
        };
        let running = {
            let mut r = run.clone();
            r.state = RunState::Running { lease, started_at: Millis(1) };
            r
        };
        let orphaned = {
            let mut r = run.clone();
            r.state = RunState::Orphaned { last: lease, since: Millis(1) };
            r
        };

        // The arbiter says the holder has gone quiet.
        view.merge_run(running.clone(), node(holder));
        view.merge_run(orphaned, arbiter);
        let orphaned_now = matches!(
            view.runs.get(&run.id).map(|r| &r.state),
            Some(RunState::Orphaned { .. })
        );
        prop_assert!(orphaned_now, "the arbiter's own orphan did not take");

        // Somebody relays `Running` at the same epoch. It counts only if the speaker is the
        // holder itself (that is the reclaim path) or the arbiter changing its mind.
        view.merge_run(running, node(relay));
        let entitled = relay == holder || node(relay) == arbiter;
        let still_orphaned = matches!(
            view.runs.get(&run.id).map(|r| &r.state),
            Some(RunState::Orphaned { .. })
        );
        prop_assert_eq!(
            still_orphaned,
            !entitled,
            "relay={} holder={}: an unentitled speaker moved the run", relay, holder
        );
    }

    /// Two grants at one epoch settle the same way on every node, from every direction.
    ///
    /// The epoch orders grants made by *one* arbiter and says nothing about grants made by two,
    /// because both bump `next()` from the same number. ADR-0002 accepts that a partition can
    /// produce two arbiters and rests on fencing to make the second writer a rejected one — and
    /// fencing can only do that if the two grants can be ordered at all. So this is that ADR's
    /// safety claim, stated as a property: whatever order the records arrive in and whoever
    /// relays them, every node names the same holder, which is what makes the loser's own
    /// `fence` refuse it.
    #[test]
    fn two_grants_at_one_epoch_never_leave_the_fleet_disagreeing(
        holders in prop::collection::vec(1u8..5, 2..4),
        relay in 1u8..6,
        order in any::<bool>(),
    ) {
        // At least two distinct holders, topped up rather than rejected: the property is about
        // two competing grants, so a draw that happens to repeat a node is not a case to throw
        // away, it is a case to finish building.
        let mut distinct: Vec<u8> = Vec::new();
        for h in holders {
            if !distinct.contains(&h) {
                distinct.push(h);
            }
        }
        for spare in 1u8..5 {
            if distinct.len() >= 2 {
                break;
            }
            if !distinct.contains(&spare) {
                distinct.push(spare);
            }
        }

        let home = node(9);
        let mut base = Run::new(
            RunId::from_bytes([7; 16]),
            spec(Restartability::Resumable),
            home,
            Millis(0),
        );
        let e = base.assign(node(distinct[0]), Millis(0), LEASE).expect("assign");
        base.started(node(distinct[0]), e, Millis(0)).expect("start");
        base.orphan(Millis(1_000)).expect("orphan");

        // One grant per candidate holder, every one made from the same record — which is what
        // two arbiters that cannot hear each other produce.
        let grants: Vec<Run> = distinct
            .iter()
            .map(|h| {
                let mut r = base.clone();
                let e = r.assign(node(*h), Millis(2_000), LEASE).expect("grant");
                r.started(node(*h), e, Millis(2_000)).expect("start");
                r
            })
            .collect();
        for g in &grants {
            prop_assert_eq!(g.epoch, grants[0].epoch, "the grants were not at one epoch");
        }

        let members: Vec<u8> = (1u8..6).chain(std::iter::once(9)).collect();
        let settle = |sequence: &[(Run, NodeId)]| {
            let mut view = view_of(node(1), &members, Millis(0));
            for (run, from) in sequence {
                view.merge_run(run.clone(), *from);
            }
            view.runs.values().next().and_then(Run::holder)
        };

        // Heard from each holder itself, and heard entirely from one relay: a record's
        // entitlement must not be what decides this, because both sides have it.
        let own: Vec<(Run, NodeId)> = grants
            .iter()
            .cloned()
            .zip(distinct.iter().map(|h| node(*h)))
            .collect();
        let relayed: Vec<(Run, NodeId)> =
            grants.iter().cloned().map(|g| (g, node(relay))).collect();

        let mut reversed = own.clone();
        reversed.reverse();
        let expected = settle(&own);
        prop_assert!(expected.is_some(), "the run vanished");
        prop_assert_eq!(settle(&reversed), expected, "reversing named another holder");
        prop_assert_eq!(settle(&relayed), expected, "a relay named another holder");
        if order {
            let mut rotated = relayed.clone();
            rotated.rotate_left(1);
            prop_assert_eq!(settle(&rotated), expected, "a rotation named another holder");
        }
    }

    /// Two nodes that heard the same things agree about the numbers.
    ///
    /// `RunProgress::absorb` is a lattice join if and only if both halves of it are: the spend
    /// counters are a field-wise max, which is one by construction, and the position is a
    /// max-register whose ordering has to be **total** — epoch, then the lowest author id, then
    /// turns, then the author's clock, then the summary itself. Every one of those tiebreaks
    /// exists because the level above it can genuinely tie, and the last one exists only for this
    /// property: two writes in one millisecond with different summaries are otherwise settled by
    /// which gossip arrived first.
    ///
    /// The numbers are the one thing in a run that cannot be re-derived. A cost that depends on
    /// the order the gossip arrived in is a bill two devices disagree about for ever, and `ps`
    /// flickering between them is the only symptom.
    ///
    /// Legs are generated, not fixed, because that is where the merge stopped being a simple
    /// max: a run granted twice has two authors, and the case that pays is two of them at *one*
    /// epoch, where nothing but the author's id can order them.
    #[test]
    fn the_order_gossip_arrives_in_does_not_change_the_numbers(
        records in prop::collection::vec(
            (0u32..4, 0u64..4, 0u32..3, 0u32..3, 0u64..4, 0u8..3, 0u64..3, 0usize..2),
            1..7,
        ),
        rotate in 0usize..7,
    ) {
        let progress: Vec<RunProgress> = records
            .iter()
            .map(|&(turns, cost_micro_usd, denials, asks, at, leg, epoch, note)| RunProgress {
                turns,
                denials,
                asks,
                cost_micro_usd,
                // Two summaries at one instant is the tie the string itself has to break.
                workspace: ["2 modified", "1 new"][note].to_string(),
                at: Millis(at),
                // Leg 0 is a record from before legs were stamped: no author, epoch 0.
                by: (leg > 0).then(|| node(leg)),
                epoch: if leg > 0 { Epoch(epoch + 1) } else { Epoch(0) },
                // Derived so that it *varies with the record* rather than being a constant the
                // merge could carry by accident: tokens travel with the position (ADR-0040), so
                // whichever position the fold settles on has to bring its own totals with it,
                // in whatever order the records arrive.
                tokens: offload_core::TokenUse {
                    output: u64::from(turns) * 100 + at,
                    messages: u64::from(turns),
                    ..offload_core::TokenUse::default()
                },
                // Per-leg spend (ADR-0067), varying with the record too, so the union has to
                // settle each leg's copies the same way whatever order they arrive in.
                legs: if leg > 0 {
                    vec![offload_core::LegSpend {
                        by: node(leg),
                        epoch: Epoch(epoch + 1),
                        base: offload_core::TokenUse::default(),
                        tokens: offload_core::TokenUse {
                            output: u64::from(turns) * 100 + at,
                            messages: u64::from(turns),
                            ..offload_core::TokenUse::default()
                        },
                        cost_micro_usd,
                    }]
                } else {
                    Vec::new()
                },
            })
            .collect();

        let fold = |order: &[RunProgress]| {
            let mut held = RunProgress::default();
            for p in order {
                held.absorb(p);
            }
            held
        };

        let mut rotated = progress.clone();
        rotated.rotate_left(rotate % progress.len());
        let mut reversed = progress.clone();
        reversed.reverse();

        let a = fold(&progress);
        let b = fold(&rotated);
        let c = fold(&reversed);
        prop_assert_eq!(&a, &b, "a rotation of the same gossip settled somewhere else");
        prop_assert_eq!(&a, &c, "reversing the same gossip settled somewhere else");
    }
}

// ---------------------------------------------------------------------------
// The bid round, where disagreement means two nodes running one run
// ---------------------------------------------------------------------------

fn bid(n: u8, score: i64, now: bool) -> Bid {
    Bid {
        run: RunId::from_bytes([7; 16]),
        node: node(n),
        epoch: Epoch::INITIAL,
        score: Score(score),
        available: if now {
            Availability::Now
        } else {
            Availability::WhenFree { behind: 1 }
        },
        submitted_at: Millis(0),
        terms: None,
    }
}

proptest! {
    /// Every node that has seen the same bids picks the same winner, whatever order they came in.
    ///
    /// This is the property that lets a bid round cost one message instead of a consensus
    /// protocol: the arbiter grants, and every other node can work out who should have won
    /// without being told. If the answer depended on arrival order, two nodes computing it
    /// would name two winners — and the failure that follows is the one this project cares
    /// about most, two agents on one repo.
    ///
    /// So the comparison has to be a *total* order, which means the tiebreak has to reach all
    /// the way down to the node id. Score ties are not exotic: two identical machines on one
    /// LAN produce them constantly.
    #[test]
    fn the_winner_does_not_depend_on_the_order_the_bids_arrived(
        raw in prop::collection::vec((1u8..6, -3i64..4, any::<bool>()), 1..6),
        rotate in 0usize..6,
    ) {
        // One bid per node, as a real round produces: a node bids once.
        let mut seen = BTreeSet::new();
        let bids: Vec<Bid> = raw
            .iter()
            .filter(|(n, _, _)| seen.insert(*n))
            .map(|&(n, s, now)| bid(n, s, now))
            .collect();
        prop_assume!(!bids.is_empty());

        let mut rotated = bids.clone();
        rotated.rotate_left(rotate % bids.len());
        let mut reversed = bids.clone();
        reversed.reverse();

        let pick = |v: &[Bid]| offload_core::bid::winner(v).map(|b| b.node);
        prop_assert_eq!(pick(&bids), pick(&rotated), "a rotation named another winner");
        prop_assert_eq!(pick(&bids), pick(&reversed), "reversing named another winner");

        // And the winner is genuinely the best offer, not merely a stable one: anybody who
        // can start now beats anybody who cannot, and score decides among those.
        let won = pick(&bids).expect("a non-empty round has a winner");
        let best = bids
            .iter()
            .max_by_key(|b| (b.available.is_now(), b.score, std::cmp::Reverse(b.node)))
            .expect("non-empty");
        prop_assert_eq!(won, best.node, "the winner was not the best bid");
    }
}

// ---------------------------------------------------------------------------
// `explain`, which is the answer to the question this system is asked most
// ---------------------------------------------------------------------------

fn constraint() -> impl Strategy<Value = Constraint> {
    let leaf = prop_oneof![
        Just(Constraint::Always),
        (0u32..6).prop_map(Constraint::MinCores),
        (0u64..6).prop_map(Constraint::MinMemoryMb),
        Just(Constraint::OnMains),
        Just(Constraint::UnmeteredNetwork),
        Just(Constraint::HasAgent(AgentKind::ClaudeCode)),
        Just(Constraint::HasTag("gpu".into())),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Constraint::All),
            prop::collection::vec(inner.clone(), 0..3).prop_map(Constraint::Any),
            inner.prop_map(|c| Constraint::Not(Box::new(c))),
        ]
    })
}

fn caps(cores: u32, mem: u64, mains: bool) -> offload_core::capability::Capabilities {
    use offload_core::capability::{Arch, Capabilities, DeviceClass, Os, PowerSource};
    let mut c = Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop);
    c.cpu_cores = cores;
    c.memory_mb = mem;
    c.power = if mains {
        PowerSource::Ac
    } else {
        PowerSource::Battery {
            percent: 50,
            charging: false,
        }
    };
    c
}

proptest! {
    /// A constraint that did not match always names something that did not hold.
    ///
    /// "Twelve nodes match, why is my run pending" is the most-asked question this system has,
    /// and `Explain::failures()` is the whole of the answer. It descends only through
    /// unsatisfied branches — right, because an `Any` that succeeded should not report its
    /// losing arms — and the risk in that is a branch whose children all *held* while the
    /// branch itself did not. A `Not` is exactly that shape. An empty failure list under a
    /// refusal is the worst output available: the run is pending, and the tool that exists to
    /// say why says nothing.
    #[test]
    fn a_refusal_always_names_a_reason(
        c in constraint(),
        cores in 0u32..8,
        mem in 0u64..8,
        mains in any::<bool>(),
    ) {
        let caps = caps(cores, mem, mains);
        let explained = c.explain(&caps);
        prop_assert_eq!(
            explained.satisfied,
            c.matches(&caps),
            "explain and matches disagreed about {:?}", c
        );
        if !explained.satisfied {
            prop_assert!(
                !explained.failures().is_empty(),
                "{:?} was refused and named nothing", c
            );
        } else {
            prop_assert!(
                explained.failures().is_empty(),
                "{:?} matched and named a failure anyway", c
            );
        }
    }

    /// The reasons a refusal gives do not depend on how the clauses were written down.
    ///
    /// `All([a, b])` and `All([b, a])` are the same requirement, so they owe the operator the
    /// same account of why it was not met. This is not pedantry about set semantics: the list
    /// comes from wherever the constraint was assembled — a repo config, a `--constraint`
    /// string, `Constraint::agent_ready` — and an answer that changed with the order would mean
    /// two nodes explaining one refusal differently.
    #[test]
    fn the_reasons_do_not_depend_on_clause_order(
        clauses in prop::collection::vec(constraint(), 2..5),
        cores in 0u32..8,
        mem in 0u64..8,
        mains in any::<bool>(),
    ) {
        let caps = caps(cores, mem, mains);
        let forward = Constraint::All(clauses.clone());
        let backward = Constraint::All(clauses.iter().rev().cloned().collect());
        prop_assert_eq!(forward.matches(&caps), backward.matches(&caps));

        let mut a = forward.explain(&caps).failures();
        let mut b = backward.explain(&caps).failures();
        a.sort();
        b.sort();
        prop_assert_eq!(a, b, "reordering the clauses changed the explanation");
    }
}

// ---------------------------------------------------------------------------
// The allowlist, which is trusted and therefore has to be right
// ---------------------------------------------------------------------------

/// Programs that run an arbitrary string, sampled from what the module lists.
///
/// A sample rather than the list itself: the point is that *spelling* does not matter, and a
/// handful of entries proves that as well as forty do while keeping the counterexamples legible.
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "python3", "perl", "node", "env", "xargs", "sudo",
];

/// The ways a command word can be written without changing which program runs.
///
/// Each of these used to produce `Risk::Scoped`, which is what `require_scoped` accepts from a
/// repository's own `.offload.toml`. Measured on `claude 2.1.238`: `Bash(/bin/sh:*)` then ran
/// `/bin/sh -c "cargo build --version"`, a command the same run was denied on its own.
fn where_it_lives() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("/bin/".to_string()),
        Just("/usr/bin/".to_string()),
        Just("./".to_string()),
        Just("../bin/".to_string()),
    ]
}

/// One command word, spelled every way that still runs the same program.
///
/// Built rather than generated-and-filtered: quoting a word that has an argument after it
/// (`"sh -c"`) is not a spelling anybody writes, and rejecting those afterwards spends the
/// generator's whole budget on cases the property was never about.
fn spelled(program: &str) -> impl Strategy<Value = String> {
    let program = program.to_string();
    (
        prop_oneof![
            Just(String::new()),
            Just("FOO=1 ".to_string()),
            Just("FOO=1 BAR=2 ".to_string()),
        ],
        where_it_lives(),
        prop_oneof![
            Just(("", false)),
            Just(("3.11", false)),
            Just(("", true)),
            Just((" -c", false)),
            Just((" --login", false)),
        ],
    )
        .prop_map(move |(assignments, path, (suffix, quoted))| {
            let word = format!("{path}{program}{suffix}");
            let word = if quoted { format!("\"{word}\"") } else { word };
            format!("{assignments}{word}")
        })
}

proptest! {
    /// However an interpreter is spelled, the allowlist calls it what it is.
    ///
    /// `require_scoped` exists for exactly one thing: a checked-in `.offload.toml` is *content*,
    /// and content must not be able to grant itself a shell. So the question is never "is this
    /// pattern in the list" but "does this pattern name a program in the list" — and the gap
    /// between those two is a path, a quote, an environment assignment, or a version number.
    #[test]
    fn a_shell_by_any_other_name_is_still_a_shell(
        word in prop::sample::select(INTERPRETERS).prop_flat_map(spelled),
    ) {
        let raw = format!("Bash({word}:*)");
        let pattern = ToolPattern::parse(&raw).expect("parses");
        prop_assert_ne!(
            pattern.risk(),
            Risk::Scoped,
            "{} reads as scoped and is a shell",
            raw
        );

        // …and the check built on it refuses the pattern, naming it, so whoever wrote the
        // `.offload.toml` can see which line was the problem.
        let list = ToolAllowlist::parse([raw.as_str()]).expect("parses");
        let refused = list.require_scoped().expect_err("a shell was accepted");
        prop_assert!(
            refused.to_string().contains(&raw),
            "the refusal has to name the pattern: {refused}"
        );
    }

    /// A build tool stays scoped, however it is spelled.
    ///
    /// The other direction, and the reason the list is a list rather than "anything with
    /// arguments". Allowing `cargo test` means trusting the repo's own build scripts — a real
    /// caveat, and a different one from handing over a shell. If this ever fails, the
    /// normalisation has started over-matching and the feature has become unusable for the
    /// thing it was built for.
    #[test]
    fn a_build_tool_is_not_mistaken_for_a_shell(
        tool in prop::sample::select(&["cargo", "make", "npm", "pnpm", "go", "just", "gradle"][..]),
        // No assignment prefix here: `FOO=1 cargo test` genuinely does leave the command after
        // it open, which is the property above's subject rather than this one's.
        path in where_it_lives(),
        subcommand in prop::sample::select(&["test", "build", "run", "check"][..]),
    ) {
        let raw = format!("Bash({path}{tool} {subcommand}:*)");
        let pattern = ToolPattern::parse(&raw).expect("parses");
        prop_assert_eq!(pattern.risk(), Risk::Scoped, "{} was refused", raw);
        prop_assert!(ToolAllowlist::parse([raw.as_str()])
            .expect("parses")
            .require_scoped()
            .is_ok());
    }

    /// A pattern survives the trip to the agent's argv and back.
    ///
    /// The allowlist is assembled from three layers — node config, the repo's `.offload.toml`,
    /// `--allow` — merged, and then handed to the agent as `--allowedTools` arguments. Anything
    /// lost or mangled in that round trip is a grant that silently is not what was written,
    /// which for this type means either a run that cannot execute its own tests or one that can
    /// execute more than it was given.
    #[test]
    fn what_goes_to_the_agent_is_what_was_written(
        raws in prop::collection::vec(
            prop_oneof![
                Just("Read".to_string()),
                Just("Edit".to_string()),
                Just("Bash(cargo test:*)".to_string()),
                Just("Bash(git status)".to_string()),
                Just("WebFetch(https://docs.rs)".to_string()),
                Just("mcp__email".to_string()),
            ],
            0..6,
        ),
    ) {
        let list = ToolAllowlist::parse(raws.iter().map(String::as_str)).expect("parses");
        let round = ToolAllowlist::parse(list.to_args()).expect("re-parses");
        prop_assert_eq!(&list, &round, "a pattern changed on the way to the agent");

        // Merging is a union that keeps order and drops duplicates, because these are layers
        // and a later layer removing an operator's grant would surprise in the wrong direction.
        prop_assert_eq!(list.merged_with(&list), list.clone(), "merge is not idempotent");
        let empty = ToolAllowlist::default();
        prop_assert_eq!(list.merged_with(&empty), list.clone());
        prop_assert_eq!(empty.merged_with(&list), list.clone());
    }

    /// A grant covers the calls it names and nothing beyond its own prefix.
    ///
    /// `covers` decides whether a permission question is worth putting to a person (ADR-0017),
    /// and a match means "let the agent apply its own rules", never "allow" — so being *wider*
    /// than the agent costs a question and can never grant anything. What it must not be is
    /// wider than its own written prefix, because that is the direction where a run stops asking
    /// about calls nobody granted.
    #[test]
    fn a_prefix_grant_covers_its_prefix_and_no_more(
        head in "[a-z]{1,6}",
        tail in "[a-z ]{0,8}",
        other in "[a-z]{1,6}",
    ) {
        let list = ToolAllowlist::parse([format!("Bash({head}:*)").as_str()]).expect("parses");
        prop_assert!(list.covers("Bash", &head));
        let extended = format!("{head}{tail}");
        prop_assert!(list.covers("Bash", &extended));
        prop_assert!(!list.covers("Read", &head), "a Bash grant covered a Read");
        if !other.starts_with(&head) {
            prop_assert!(
                !list.covers("Bash", &other),
                "`Bash({}:*)` covered `{}`", head, other
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Membership, where a chain that verifies must never grant more than it was issued
// ---------------------------------------------------------------------------

fn signing(seed: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
}

fn grant_set(bits: u8) -> BTreeSet<Grant> {
    let mut out = BTreeSet::new();
    for (i, grant) in [
        Grant::Submit,
        Grant::Deliver,
        Grant::HostRuns,
        Grant::Approve,
    ]
    .into_iter()
    .enumerate()
    {
        if bits & (1 << i) != 0 {
            out.insert(grant);
        }
    }
    out
}

fn membership_terms(fleet: FleetId, member: NodeId, grants: BTreeSet<Grant>) -> Terms {
    Terms {
        grants,
        ..Terms::joining(fleet, member, "device", Millis(1_000))
    }
}

proptest! {
    /// A chain that verifies never grants more than the fleet key allowed.
    ///
    /// The one property worth having about membership, because everything else in the design
    /// leans on it: a certificate is public, copyable, verified offline at first contact, and
    /// what it says is what a peer acts on. ADR-0012's delegation bounds *who* may issue and its
    /// type sketch has a `may_issue` that never reached the code, so what an approver could put
    /// in a certificate was unbounded — and `HostRuns` is the grant mitigation 1 keeps behind
    /// the passphrase precisely because it means "runs agents on your repositories with your
    /// credentials".
    ///
    /// Stated as the invariant rather than as the fix: whatever an approver signs, a verifying
    /// peer must conclude no more than `{submit, deliver}` plus whatever the *fleet key* granted
    /// this same member. Anything else, in any combination, must fail to verify.
    #[test]
    fn an_approver_cannot_sign_a_grant_the_fleet_key_did_not_make(
        claimed in 0u8..16,
        authorised in prop::option::of(0u8..16),
        // Whether the authority names this member or somebody else, and whether the fleet key
        // or the approver signed it. Only one of the four combinations is an authority.
        same_member in any::<bool>(),
        fleet_signed in any::<bool>(),
    ) {
        let fleet_key = signing(1);
        let fleet = FleetId::from_bytes(fleet_key.verifying_key().to_bytes());
        let approver_key = signing(3);
        let approver = NodeId::from_bytes(approver_key.verifying_key().to_bytes());
        let member = NodeId::from_bytes(signing(4).verifying_key().to_bytes());
        let other = NodeId::from_bytes(signing(5).verifying_key().to_bytes());
        let delegation =
            Delegation::issue(&fleet_key, fleet, approver, Millis(0), Millis(86_400_000), 1);
        let issuer = Issuer::Approver { node: approver };

        let authority = authorised.map(|bits| {
            let subject = if same_member { member } else { other };
            let terms = membership_terms(fleet, subject, grant_set(bits));
            Box::new(if fleet_signed {
                MembershipCert::issue(&fleet_key, Issuer::Fleet, terms)
            } else {
                MembershipCert::issue(&approver_key, issuer, terms)
            })
        });

        let claimed_grants = grant_set(claimed);
        let cert = MembershipCert::issue(
            &approver_key,
            issuer,
            Terms {
                authority: authority.clone(),
                ..membership_terms(fleet, member, claimed_grants.clone())
            },
        );

        // What the fleet key actually allowed: the door, plus a real authority's grants.
        let mut allowed = offload_core::default_grants();
        if same_member && fleet_signed {
            if let Some(bits) = authorised {
                allowed.extend(grant_set(bits));
            }
        }
        let within = claimed_grants.is_subset(&allowed);

        let verdict = cert.verify(fleet, Some(&delegation), Millis(2_000));
        prop_assert_eq!(
            verdict.is_ok(),
            within,
            "claimed {:?}, allowed {:?}, verdict {:?}",
            claimed_grants,
            allowed,
            verdict
        );

        // And whatever a peer concludes about a certificate that verified is bounded by the
        // same set — the check has to bind `effective_grants`, not merely `verify`.
        if verdict.is_ok() {
            for grant in cert.effective_grants(Millis(2_000)) {
                prop_assert!(
                    allowed.contains(&grant),
                    "a verified certificate granted {} that nobody issued",
                    grant
                );
            }
        }
    }

    /// Renewal moves the clock and nothing else, however many times it happens.
    ///
    /// The other half of the rule above: what makes it safe for an approver to restate a grant
    /// it could not make is that restating cannot widen. Checked over a chain rather than one
    /// hop, because the interesting failure is a chain that gains something on its fourth link —
    /// and because a renewal that carried its immediate predecessor rather than the fleet-signed
    /// root would grow a certificate without bound on a host that has been up for a year.
    #[test]
    fn a_chain_of_renewals_neither_widens_nor_grows(
        granted in 0u8..16,
        hops in 1usize..6,
    ) {
        let fleet_key = signing(1);
        let fleet = FleetId::from_bytes(fleet_key.verifying_key().to_bytes());
        let approver_key = signing(3);
        let approver = NodeId::from_bytes(approver_key.verifying_key().to_bytes());
        let member = NodeId::from_bytes(signing(4).verifying_key().to_bytes());
        let day = Millis(86_400_000);
        let delegation =
            Delegation::issue(&fleet_key, fleet, approver, Millis(0), Millis(day.0 * 400), 1);

        let root = MembershipCert::issue(
            &fleet_key,
            Issuer::Fleet,
            membership_terms(fleet, member, grant_set(granted)),
        );
        let mut cert = root.clone();
        for hop in 1..=hops {
            let now = Millis(day.0 * hop as u64);
            cert = MembershipCert::issue(
                &approver_key,
                Issuer::Approver { node: approver },
                cert.renewal(now, day),
            );
            prop_assert_eq!(
                cert.verify(fleet, Some(&delegation), now + Millis(1)),
                Ok(()),
                "hop {} did not verify",
                hop
            );
            prop_assert_eq!(&cert.grants, &root.grants, "hop {} widened the grants", hop);
            prop_assert_eq!(
                cert.authority.as_deref(),
                Some(&root),
                "hop {} pointed somewhere other than what the fleet key signed",
                hop
            );
        }
    }
}
