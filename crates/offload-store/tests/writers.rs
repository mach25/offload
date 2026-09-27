//! What a writer may assume about the row it is writing.
//!
//! `save_run` serialises a whole `Run` into a whole row. That is not a criticism of it — the
//! JSON is authoritative and a partial write has no meaning — but it makes every caller's copy
//! of a run a *claim about every field*, including the fields that caller never looked at. A
//! copy that has been sitting therefore does not save a change; it restores a moment.
//!
//! One writer in the tree had this written down (`Supervisor::replicate`: "turns keep happening
//! while bytes are in flight, and writing back a stale checkpoint would undo one") and re-read
//! before it wrote. The others did not, and nothing here could have told them apart, because the
//! window needs no `await` in it: two tokio tasks and one store are enough.
//!
//! So the claim these check is `Store::update_run`'s: **a change applied under the store's own
//! lock cannot lose a write that landed while the caller was thinking.** With, beside it, the
//! characterisation of `save_run` that says why the other one has to exist.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use offload_core::{
    AgentKind, AgentWork, Checkpoint, Constraint, Millis, NodeId, PermissionMode, Restartability,
    Run, RunId, RunSpec, ToolAllowlist, Work, WorkspaceSpec,
};
use offload_store::Store;
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "offload-writers-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

fn spec() -> RunSpec {
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
    }
}

fn run_id(n: u8) -> RunId {
    let mut id = [0u8; 16];
    id[0] = n.wrapping_add(1);
    RunId::from_bytes(id)
}

fn a_run(home: NodeId) -> Run {
    Run::new(run_id(0), spec(), home, Millis(0))
}

/// The shape a real capture produces, as far as this file cares: a transcript that must survive.
fn a_checkpoint(store: &Store, turn: u32) -> Checkpoint {
    let byte = u8::try_from(turn % 251).expect("small");
    let transcript = store.put_blob(&[byte; 64]).expect("put");
    Checkpoint {
        replicas: BTreeSet::new(),
        session_id: Some("s".into()),
        transcript,
        bundle: None,
        patch: None,
        base_commit: "c0ffee".into(),
        turns: turn,
        taken_at: Millis(turn.into()),
        agent_version: "v".into(),
    }
}

/// What `save_run` means, stated so that the reason for `update_run` is a test rather than a
/// comment: it writes **every** field, from the copy it was handed.
///
/// The interleave is the ordinary one and needs no threads to describe. A task reads a run; a
/// second task records a checkpoint and finishes it; the first writes back what it read. The
/// second task's work is not merged and not refused — it is simply not there any more, and no
/// call returned an error.
#[test]
fn a_save_writes_back_the_moment_it_read_and_not_the_change_it_made() {
    let dir = temp_dir("whole-row");
    let store = Store::open(&dir).expect("open");
    let me = NodeId::from_bytes([1; 32]);

    let mut run = a_run(me);
    let epoch = run.assign(me, Millis(1), Millis(60_000)).expect("assign");
    run.started(me, epoch, Millis(2)).expect("start");
    store.save_run(&run).expect("save");

    // One task reads it and goes off to do something slow.
    let mut sitting = store.load_run(run.id).expect("load").expect("there");

    // Another records a turn's checkpoint.
    let checkpoint = a_checkpoint(&store, 7);
    store
        .update_run(run.id, |run| {
            run.record_checkpoint(me, epoch, checkpoint.clone())
                .expect("checkpoint");
        })
        .expect("update")
        .expect("there");

    // The first one comes back and renews its lease, which is one field of many.
    sitting
        .renew(me, epoch, Millis(3), Millis(60_000))
        .expect("renew");
    store.save_run(&sitting).expect("save");

    let stored = store.load_run(run.id).expect("load").expect("there");
    assert!(
        stored.checkpoint.is_none(),
        "this is what a whole-row write does, and why `update_run` exists"
    );
}

/// …and the same interleave through `update_run`, which is the fix: the change is applied to the
/// row as it stands, so the checkpoint is still there and the lease still moved.
#[test]
fn an_update_applies_the_change_to_the_row_as_it_stands() {
    let dir = temp_dir("update");
    let store = Store::open(&dir).expect("open");
    let me = NodeId::from_bytes([1; 32]);

    let mut run = a_run(me);
    let epoch = run.assign(me, Millis(1), Millis(60_000)).expect("assign");
    run.started(me, epoch, Millis(2)).expect("start");
    store.save_run(&run).expect("save");

    let before = store.load_run(run.id).expect("load").expect("there");
    let was = before.state.lease().map(|l| l.expires_at).expect("a lease");

    let checkpoint = a_checkpoint(&store, 7);
    store
        .update_run(run.id, |run| {
            run.record_checkpoint(me, epoch, checkpoint.clone())
                .expect("checkpoint");
        })
        .expect("update")
        .expect("there");

    store
        .update_run(run.id, |run| {
            run.renew(me, epoch, Millis(90_000), Millis(60_000))
                .expect("renew");
        })
        .expect("update")
        .expect("there");

    let stored = store.load_run(run.id).expect("load").expect("there");
    assert_eq!(
        stored.checkpoint.map(|c| c.turns),
        Some(7),
        "a lease renewal must not undo a turn"
    );
    assert!(
        stored.state.lease().map(|l| l.expires_at) > Some(was),
        "…and must still renew the lease"
    );
}

/// A run that is not there is `None`, which is a different answer from a change that did nothing.
///
/// Worth pinning because the caller that cares is the heartbeat: a run released to another node
/// between the listing and the renewal is not an error to warn about, and a store that could not
/// be read is.
#[test]
fn updating_a_run_that_is_not_there_says_so() {
    let dir = temp_dir("absent");
    let store = Store::open(&dir).expect("open");
    let seen = store
        .update_run(run_id(9), |run| run.epoch)
        .expect("no error");
    assert!(seen.is_none());
}

proptest! {
    /// Concurrent writers through `update_run` lose nothing.
    ///
    /// Real threads on one store, because that is the arrangement the daemon has: the heartbeat
    /// tick, the pump recording a turn, the checkpoint task and the gossip loop all hold a
    /// clone of this store and none of them coordinates with the others. Each thread bumps the
    /// same counter through a read-modify-write, and the total is what nobody dropped.
    ///
    /// The same loop over `save_run` is what the tree used to do, and it loses on almost every
    /// run — but asserting *that* would be asserting a race, so it is the deterministic
    /// characterisation above that records it.
    #[test]
    fn concurrent_updates_lose_nothing(
        threads in 2usize..6,
        each in 1u32..20,
    ) {
        let dir = temp_dir("concurrent");
        let store = Store::open(&dir).expect("open");
        let run = a_run(NodeId::from_bytes([1; 32]));
        store.save_run(&run).expect("save");

        std::thread::scope(|scope| {
            for _ in 0..threads {
                let store = store.clone();
                let id = run.id;
                scope.spawn(move || {
                    for _ in 0..each {
                        store
                            .update_run(id, |run| {
                                run.spec_rev = run.spec_rev.saturating_add(1);
                                run.spec.priority = run.spec.priority.saturating_add(1);
                            })
                            .expect("update")
                            .expect("there");
                    }
                });
            }
        });

        let stored = store.load_run(run.id).expect("load").expect("there");
        let expected = u32::try_from(threads).expect("small") * each;
        prop_assert_eq!(stored.spec_rev, expected);
        prop_assert_eq!(stored.spec.priority, i32::try_from(expected).expect("small"));
    }
}
