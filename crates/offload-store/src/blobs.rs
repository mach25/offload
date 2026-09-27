//! Content-addressed storage for the things a migration has to move.
//!
//! Transcripts, git bundles, dirty patches. Keyed by BLAKE3 hash, which gives dedup and
//! integrity for free: the same repo bundle shared across runs is stored once, and a blob
//! that arrives corrupted over the wire fails to match its own name.
//!
//! Bytes live on disk under `<state>/blobs/`, sharded by the first hash byte. Only the
//! index lives in SQLite — a git bundle can be tens of megabytes, and that is not what a
//! database row is for.

use crate::{now_ms, StoreError};
use offload_core::BlobHash;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Hash bytes without storing them. Useful for checking whether a fetch is even needed.
#[must_use]
pub fn hash(bytes: &[u8]) -> BlobHash {
    BlobHash::from_bytes(*blake3::hash(bytes).as_bytes())
}

/// Where a blob's bytes live, relative to the blob root.
///
/// Sharded on the first byte so a fleet's worth of checkpoints does not land in one
/// directory — some filesystems cope badly, and every `ls` becomes painful.
#[must_use]
pub fn blob_path(root: &Path, hash: BlobHash) -> PathBuf {
    let hex = hash.to_string();
    root.join(&hex[..2]).join(hex)
}

/// Store `bytes` and return its hash.
///
/// Idempotent: storing the same bytes twice is one file and one row. Writes to a temporary
/// file and renames, so a crash mid-write cannot leave a blob whose contents do not match
/// its name — a partially written checkpoint that still looks valid is the worst outcome
/// available here.
pub fn put(conn: &Connection, root: &Path, bytes: &[u8]) -> Result<BlobHash, StoreError> {
    let digest = hash(bytes);
    let path = blob_path(root, digest);

    if !path.is_file() {
        let parent = path.parent().ok_or_else(|| StoreError::Io {
            path: path.clone(),
            reason: "blob path has no parent".into(),
        })?;
        std::fs::create_dir_all(parent).map_err(|e| StoreError::Io {
            path: parent.to_path_buf(),
            reason: e.to_string(),
        })?;

        let temp = parent.join(format!(".{digest}.partial"));
        std::fs::write(&temp, bytes).map_err(|e| StoreError::Io {
            path: temp.clone(),
            reason: e.to_string(),
        })?;
        std::fs::rename(&temp, &path).map_err(|e| StoreError::Io {
            path: path.clone(),
            reason: e.to_string(),
        })?;
    }

    let now = now_ms();
    conn.execute(
        "INSERT INTO blobs (hash, size_bytes, created_at_ms, last_used_ms)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(hash) DO UPDATE SET last_used_ms = ?3",
        rusqlite::params![
            digest.as_bytes().as_slice(),
            i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            now
        ],
    )?;

    Ok(digest)
}

/// Read a blob back, verifying it still hashes to its own name.
///
/// The verification is the point of content addressing and costs a hash of data we are
/// reading anyway. Silent bit-rot in a checkpoint would resume an agent into a corrupted
/// conversation, which is far worse than reporting that the blob is gone.
pub fn get(conn: &Connection, root: &Path, digest: BlobHash) -> Result<Vec<u8>, StoreError> {
    let path = blob_path(root, digest);
    let bytes = std::fs::read(&path).map_err(|_| StoreError::MissingBlob(digest.to_string()))?;

    let actual = hash(&bytes);
    if actual != digest {
        return Err(StoreError::CorruptBlob {
            expected: digest.to_string(),
            actual: actual.to_string(),
        });
    }

    conn.execute(
        "UPDATE blobs SET last_used_ms = ?2 WHERE hash = ?1",
        rusqlite::params![digest.as_bytes().as_slice(), now_ms()],
    )?;
    Ok(bytes)
}

/// Is this blob already here? Answers "do I need to fetch it" without reading it.
#[must_use]
pub fn has(root: &Path, digest: BlobHash) -> bool {
    blob_path(root, digest).is_file()
}

/// How big this blob is, without reading it.
///
/// From the file rather than the `blobs` row, for the reason `has` looks at the file: the bytes
/// on disk are the thing, and a row without them is a blob this node does not have.
#[must_use]
pub fn size(root: &Path, digest: BlobHash) -> Option<u64> {
    std::fs::metadata(blob_path(root, digest))
        .ok()
        .map(|m| m.len())
}

/// Total bytes held.
pub fn total_size(conn: &Connection) -> Result<u64, StoreError> {
    let size: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM blobs",
        [],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(size).unwrap_or(0))
}

/// Delete blobs no run references and unused for `older_than_ms`.
///
/// Deliberately conservative and deliberately manual. Reclaiming a checkpoint that a run
/// still needs turns a resumable run into a lost one, so this only removes blobs nothing
/// points at — and blobs shared across nodes need a stronger rule than "nothing local points at
/// it": a replica pushed here for a peer's run is referenced by that peer's *record*, so it is
/// safe exactly while this node holds one (ADR-0016).
///
/// **The references are asked of the runs, not looked for in their text.** They used to be
/// matched with `instr(run_json, lower(hex(blobs.hash)))`, on the reasoning that "a substring
/// match cannot miss one — it can only be over-cautious, which is the safe direction". It was
/// the exact opposite: `BlobHash` derives serde over `[u8; 32]`, so a stored run spells its
/// transcript `[163,188,121,…]` and never as hex. The match could not *hit* one. Every blob in
/// the store looked unreferenced, so this deleted the checkpoint of a run that was still
/// running — measured, and it removed both blobs in a two-blob store where one was live.
///
/// It survived because its test hand-wrote `{"transcript":"<hex>"}`, a shape `save_run` has
/// never produced. The same hazard is called out in the schema for `fleet_events.subject`, which
/// is denormalised out of the JSON precisely because that JSON "happens to spell node ids as
/// arrays of integers" — one table over, in the same file.
pub fn collect_garbage(
    conn: &Connection,
    root: &Path,
    older_than_ms: u64,
) -> Result<usize, StoreError> {
    // `<=`, not `<`: everything written in this millisecond shares a timestamp, so a
    // strict comparison makes `collect_garbage(0)` — "collect anything unused for at
    // least no time at all" — silently collect nothing.
    let cutoff = now_ms().saturating_sub(i64::try_from(older_than_ms).unwrap_or(i64::MAX));

    // Asked before anything is deleted, and fatal if a single run will not decode: a run whose
    // references are *unknown* is not a run with none.
    let referenced = crate::runs::referenced_blobs(conn)?;

    let mut stmt = conn.prepare("SELECT hash FROM blobs WHERE last_used_ms <= ?1")?;
    let candidates: Vec<Vec<u8>> = stmt
        .query_map([cutoff], |row| row.get(0))?
        .filter_map(Result::ok)
        .collect();

    let mut removed = 0;
    for raw in candidates {
        let Ok(bytes): Result<[u8; 32], _> = raw.clone().try_into() else {
            continue;
        };
        let digest = BlobHash::from_bytes(bytes);
        if referenced.contains(&digest) {
            continue;
        }
        let _ = std::fs::remove_file(blob_path(root, digest));
        conn.execute("DELETE FROM blobs WHERE hash = ?1", [&raw])?;
        removed += 1;
    }

    if removed > 0 {
        tracing::info!(removed, "collected unreferenced blobs");
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn store(tag: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "offload-blobs-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().elapsed().map(|d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Store::open(&dir).expect("open");
        (store, dir)
    }

    #[test]
    fn round_trips_and_dedups() {
        let (store, dir) = store("roundtrip");
        let a = store.put_blob(b"hello checkpoint").expect("put");
        let b = store.put_blob(b"hello checkpoint").expect("put again");
        assert_eq!(a, b, "same bytes, same name");

        assert_eq!(store.get_blob(a).expect("get"), b"hello checkpoint");
        assert_eq!(store.blob_count().expect("count"), 1, "stored once");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupted_blob_is_reported_not_returned() {
        // The reason to verify on read: resuming an agent into a silently corrupted
        // transcript is worse than telling the caller the checkpoint is gone.
        let (store, dir) = store("corrupt");
        let digest = store.put_blob(b"original contents").expect("put");

        std::fs::write(blob_path(&store.blob_root(), digest), b"tampered").expect("tamper");

        assert!(matches!(
            store.get_blob(digest),
            Err(StoreError::CorruptBlob { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_blob_is_a_clean_error() {
        let (store, dir) = store("missing");
        let never_stored = hash(b"not here");
        assert!(matches!(
            store.get_blob(never_stored),
            Err(StoreError::MissingBlob(_))
        ));
        assert!(!has(&store.blob_root(), never_stored));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hashes_match_blake3() {
        // Content addressing is only useful if two nodes independently agree on the name.
        let expected = blake3::hash(b"shared bytes");
        assert_eq!(hash(b"shared bytes").as_bytes(), expected.as_bytes());
    }

    #[test]
    fn blobs_are_sharded_rather_than_piled_in_one_directory() {
        let (store, dir) = store("shard");
        let digest = store.put_blob(b"x").expect("put");
        let path = blob_path(&store.blob_root(), digest);
        let hex = digest.to_string();

        assert!(path.ends_with(format!("{}/{hex}", &hex[..2])), "{path:?}");
        assert!(path.is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A run with a real checkpoint, stored the way `save_run` stores one.
    fn run_with(transcript: offload_core::BlobHash) -> offload_core::Run {
        use offload_core::{
            AgentKind, AgentWork, Checkpoint, Constraint, Millis, NodeId, PermissionMode,
            Restartability, Run, RunId, RunSpec, ToolAllowlist, Work, WorkspaceSpec,
        };
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
        let mut run = Run::new(
            RunId::from_bytes([1; 16]),
            spec,
            NodeId::from_bytes([2; 32]),
            Millis(0),
        );
        run.checkpoint = Some(Checkpoint {
            session_id: Some("s".into()),
            transcript,
            bundle: None,
            patch: None,
            base_commit: "abc".into(),
            turns: 1,
            taken_at: Millis(1),
            agent_version: "2.1.0".into(),
            replicas: std::collections::BTreeSet::new(),
        });
        run
    }

    #[test]
    fn garbage_collection_spares_blobs_a_run_still_references() {
        // Reclaiming a checkpoint a run still needs turns a resumable run into a lost one.
        //
        // Through `save_run`, which is the point. This test used to insert a row with the JSON
        // `{"transcript":"<hex>"}` — a shape nothing in the tree produces — and passed while the
        // collector deleted the checkpoint of every live run: `BlobHash` derives serde over
        // `[u8; 32]`, so a stored run spells its transcript `[163,188,121,…]` and the reference
        // test looked for hex. A test that builds its own fixture by hand is a test that can
        // agree with the code about a world neither of them lives in.
        let (store, dir) = store("gc");
        let referenced = store.put_blob(b"a checkpoint transcript").expect("put");
        let orphan = store.put_blob(b"nothing points at this").expect("put");
        store.save_run(&run_with(referenced)).expect("save");

        let removed = store.collect_garbage(0).expect("gc");
        assert_eq!(removed, 1, "only the orphan goes");
        assert!(
            store.get_blob(referenced).is_ok(),
            "referenced blob survives"
        );
        assert!(store.get_blob(orphan).is_err(), "orphan collected");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_run_that_will_not_decode_stops_the_collection_rather_than_losing_its_blobs() {
        // Unknown references are not absent ones. A row this version cannot read — written by a
        // newer build, or corrupted — would otherwise have every blob it points at collected,
        // which is the one outcome this function exists to avoid.
        let (store, dir) = store("gc-undecodable");
        let orphan = store.put_blob(b"nothing points at this").expect("put");
        store
            .conn()
            .execute(
                "INSERT INTO runs (id, state, terminal, epoch, home_node, created_at_ms,
                                   updated_at_ms, run_json)
                 VALUES (?1, 'running', 0, 1, ?2, 0, 0, ?3)",
                rusqlite::params![
                    [9u8; 16].as_slice(),
                    [2u8; 32].as_slice(),
                    r#"{"from":"a version that knew more than this one"}"#,
                ],
            )
            .expect("insert run");

        assert!(
            store.collect_garbage(0).is_err(),
            "a store it cannot read is a store it must not tidy"
        );
        assert!(store.get_blob(orphan).is_ok(), "nothing was collected");
        std::fs::remove_dir_all(&dir).ok();
    }
}
