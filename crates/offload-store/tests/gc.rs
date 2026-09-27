//! What the blob collector must never do, over stores nobody arranged.
//!
//! One claim, and the whole of ADR-0002's phase-2 machinery rests on it: **a blob a run still
//! references is never collected**. A checkpoint is a transcript plus a bundle plus a patch, and
//! a run that loses any of them stops being resumable — which turns the migration this project
//! exists to perform into the data loss it exists to prevent.
//!
//! Worth stating as a property rather than as a case because the failure was invisible for two
//! phases behind a test that agreed with the code about a world neither lived in: the fixture
//! hand-wrote `{"transcript":"<hex>"}` and the collector looked for hex, while every *real* run
//! spells its transcript `[163,188,121,…]`. Both halves were consistent, and both were fiction.
//! A property that builds its runs the way the daemon does cannot make that mistake.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use offload_core::{
    AgentKind, AgentWork, BlobHash, Checkpoint, Constraint, Millis, NodeId, PermissionMode,
    Restartability, Run, RunId, RunSpec, ToolAllowlist, Work, WorkspaceSpec,
};
use offload_store::Store;
use proptest::prelude::*;
use std::collections::BTreeSet;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "offload-gc-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

/// Which of a checkpoint's three blobs a run has: a transcript always, a bundle and a patch
/// only sometimes, which is exactly the shape a real capture produces.
#[derive(Debug, Clone, Copy)]
struct Shape {
    transcript: u8,
    bundle: Option<u8>,
    patch: Option<u8>,
}

fn shape() -> impl Strategy<Value = Shape> {
    (0u8..6, prop::option::of(0u8..6), prop::option::of(0u8..6)).prop_map(
        |(transcript, bundle, patch)| Shape {
            transcript,
            bundle,
            patch,
        },
    )
}

proptest! {
    /// Exactly the blobs no run references are collected, and nothing else.
    ///
    /// Both directions, because each failure is its own kind of bad. Collecting a referenced
    /// blob loses work that cannot be recovered. *Not* collecting an unreferenced one is only
    /// wasted disk — but a collector that spares everything is one nobody will trust to run, and
    /// the version this replaces spared nothing at all while claiming to be over-cautious.
    #[test]
    fn a_blob_a_run_references_is_never_collected(
        shapes in prop::collection::vec(prop::option::of(shape()), 1..5),
        checkpointed in prop::collection::vec(any::<bool>(), 1..5),
    ) {
        let dir = temp_dir("referenced");
        let store = Store::open(&dir).expect("open");

        // Six distinct blobs to draw from, so runs share them the way content addressing means
        // they do — a transcript captured twice at the same turn is one blob.
        let blobs: Vec<BlobHash> = (0..6u8)
            .map(|i| store.put_blob(&[i; 64]).expect("put"))
            .collect();

        let mut referenced: BTreeSet<BlobHash> = BTreeSet::new();
        for (i, shape) in shapes.iter().enumerate() {
            let mut id = [0u8; 16];
            id[0] = u8::try_from(i).unwrap_or(0).wrapping_add(1);
            let mut run = Run::new(
                RunId::from_bytes(id),
                spec(),
                NodeId::from_bytes([2; 32]),
                Millis(0),
            );
            // A run with no checkpoint references nothing, which is most runs most of the time.
            let keep = checkpointed.get(i).copied().unwrap_or(true);
            if let (Some(shape), true) = (shape, keep) {
                let transcript = blobs[usize::from(shape.transcript)];
                let bundle = shape.bundle.map(|b| blobs[usize::from(b)]);
                let patch = shape.patch.map(|p| blobs[usize::from(p)]);
                referenced.insert(transcript);
                referenced.extend(bundle);
                referenced.extend(patch);
                run.checkpoint = Some(Checkpoint {
                    session_id: Some("s".into()),
                    transcript,
                    bundle,
                    patch,
                    base_commit: "abc".into(),
                    turns: 1,
                    taken_at: Millis(1),
                    agent_version: "2.1.0".into(),
                    replicas: BTreeSet::new(),
                });
            }
            store.save_run(&run).expect("save");
        }

        let removed = store.collect_garbage(0).expect("gc");
        prop_assert_eq!(
            removed,
            blobs.len() - referenced.len(),
            "collected {} of {} blobs with {} referenced",
            removed,
            blobs.len(),
            referenced.len()
        );
        for blob in &blobs {
            let survived = store.get_blob(*blob).is_ok();
            prop_assert_eq!(
                survived,
                referenced.contains(blob),
                "blob {} survived={} referenced={}",
                blob,
                survived,
                referenced.contains(blob)
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Collecting twice collects nothing the second time, and a survivor survives for ever.
    ///
    /// The property a person actually relies on when they put this on a timer: a collector whose
    /// output depends on how often it has run is one that eventually reaches a blob it should
    /// not. Also the cheapest check that nothing about the *first* pass — a touched
    /// `last_used_ms`, a deleted row — makes a referenced blob look collectable on the second.
    #[test]
    fn collecting_is_idempotent(
        transcript in 0u8..4,
        passes in 1usize..5,
    ) {
        let dir = temp_dir("idempotent");
        let store = Store::open(&dir).expect("open");
        let blobs: Vec<BlobHash> = (0..4u8)
            .map(|i| store.put_blob(&[i; 32]).expect("put"))
            .collect();
        let kept = blobs[usize::from(transcript)];

        let mut run = Run::new(
            RunId::from_bytes([1; 16]),
            spec(),
            NodeId::from_bytes([2; 32]),
            Millis(0),
        );
        run.checkpoint = Some(Checkpoint {
            session_id: Some("s".into()),
            transcript: kept,
            bundle: None,
            patch: None,
            base_commit: "abc".into(),
            turns: 1,
            taken_at: Millis(1),
            agent_version: "2.1.0".into(),
            replicas: BTreeSet::new(),
        });
        store.save_run(&run).expect("save");

        let first = store.collect_garbage(0).expect("gc");
        prop_assert_eq!(first, blobs.len() - 1);
        for _ in 1..passes {
            prop_assert_eq!(store.collect_garbage(0).expect("gc"), 0, "it kept going");
            prop_assert!(store.get_blob(kept).is_ok(), "the survivor stopped surviving");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// The case that produces almost all of the garbage, driven through the writer the daemon uses.
///
/// `Run::record_checkpoint` **replaces** the checkpoint, so every turn boundary after the first
/// orphans the previous transcript — and a transcript is the whole conversation so far, which
/// makes the waste quadratic in the conversation's size rather than linear. Measured on a real
/// daemon before ADR-0022: one six-turn run left six blobs totalling 851 KB, of which the
/// surviving checkpoint referenced one, at 243 KB.
///
/// A case rather than a property because the shape *is* the point: it is not "some blobs are
/// unreferenced", it is "each checkpoint strands the one before it, and the last one is the only
/// one anybody can still reach". Built by recording checkpoints in sequence, because a fixture
/// that assembled the end state by hand would be agreeing with the code about a world neither
/// lives in — which is the mistake this file was written after.
#[test]
fn every_checkpoint_but_the_last_is_garbage() {
    let dir = temp_dir("superseded");
    let store = Store::open(&dir).expect("open");
    let holder = NodeId::from_bytes([2; 32]);

    let mut run = Run::new(RunId::from_bytes([7; 16]), spec(), holder, Millis(0));
    let epoch = run
        .assign(holder, Millis(0), Millis(60_000))
        .expect("assign");
    run.started(holder, epoch, Millis(0)).expect("start");

    let mut written = Vec::new();
    for turn in 1..=6u32 {
        // Each capture stores the conversation *so far*, so no two are the same bytes and the
        // store cannot dedupe them into one blob.
        let transcript = store
            .put_blob(&vec![b'x'; 1_000 * turn as usize])
            .expect("put");
        written.push(transcript);
        run.record_checkpoint(
            holder,
            epoch,
            Checkpoint {
                session_id: Some("s".into()),
                transcript,
                bundle: None,
                patch: None,
                base_commit: "abc".into(),
                turns: turn,
                taken_at: Millis(u64::from(turn)),
                agent_version: "2.1.0".into(),
                replicas: BTreeSet::new(),
            },
        )
        .expect("checkpoint");
        store.save_run(&run).expect("save");
    }

    let last = *written.last().expect("six of them");
    assert_eq!(
        store.collect_garbage(0).expect("gc"),
        5,
        "five superseded checkpoints, and nothing else"
    );
    assert!(
        store.get_blob(last).is_ok(),
        "the checkpoint the record names is the one recovery asks for, and it must survive"
    );
    for stranded in &written[..5] {
        assert!(
            store.get_blob(*stranded).is_err(),
            "a superseded transcript is unreachable by construction: recovery fetches the blobs \
             the *record* names, so keeping it protects nothing"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// …and the last one is garbage too, the moment nothing can resume from it.
///
/// The sibling of the test above and the correction to what it measured. ADR-0022 collected the
/// *superseded* checkpoints and left the surviving one, which is the biggest of them — so a real
/// daemon settled at exactly one blob per completed run, 472 KB apiece, on the home node, on the
/// leg that ran it, and on every peer that took a replica. That number was read as success.
///
/// It is unreachable by every path there is: `Supervisor::resume` refuses `Completed` and
/// `Cancelled` **by name**, because they are decisions rather than accidents, and nothing else
/// fetches a checkpoint at all. `Failed` is the exception and the reason the collector asks
/// `Run::resumable_checkpoint` rather than `is_terminal` — it is the one terminal state that
/// reopens, so its checkpoint is the one whose loss is unrecoverable.
///
/// Driven through the transitions rather than by writing a state into a fixture: the state and
/// the checkpoint have to be set by the same code the daemon runs, or the test agrees with the
/// query about a world neither lives in.
#[test]
fn the_last_checkpoint_goes_too_once_nothing_can_resume_from_it() {
    for (name, stops_for_good) in [("completed", true), ("cancelled", true), ("failed", false)] {
        let dir = temp_dir(name);
        let store = Store::open(&dir).expect("open");
        let holder = NodeId::from_bytes([2; 32]);

        let mut run = Run::new(RunId::from_bytes([9; 16]), spec(), holder, Millis(0));
        let epoch = run
            .assign(holder, Millis(0), Millis(60_000))
            .expect("assign");
        run.started(holder, epoch, Millis(0)).expect("start");
        let transcript = store.put_blob(b"the whole conversation").expect("put");
        run.record_checkpoint(
            holder,
            epoch,
            Checkpoint {
                session_id: Some("s".into()),
                transcript,
                bundle: None,
                patch: None,
                base_commit: "abc".into(),
                turns: 6,
                taken_at: Millis(6),
                agent_version: "2.1.0".into(),
                replicas: BTreeSet::new(),
            },
        )
        .expect("checkpoint");
        store.save_run(&run).expect("save");

        // Still running: the checkpoint is what a migration would carry.
        assert_eq!(store.collect_garbage(0).expect("gc"), 0, "{name}: mid-run");

        match name {
            "completed" => run.complete(holder, epoch, Millis(10)).expect("complete"),
            "cancelled" => run.cancel(Millis(10)).expect("cancel"),
            _ => run
                .fail(holder, epoch, "the agent died", Millis(10))
                .expect("fail"),
        }
        store.save_run(&run).expect("save");

        assert_eq!(
            store.collect_garbage(0).expect("gc"),
            usize::from(stops_for_good),
            "{name}: a checkpoint is collectable exactly when nothing can start an agent from it"
        );
        assert_eq!(
            store.get_blob(transcript).is_ok(),
            !stops_for_good,
            "{name}: the bytes follow the same rule as the reference"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
