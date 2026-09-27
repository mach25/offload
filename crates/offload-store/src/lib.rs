//! Durable node state: the run registry, the event log, and the blob store.
//!
//! SQLite per ADR-0009. The deciding argument was inspectability — when a run is stuck at
//! midnight, `sqlite3 ~/.offload/state.db 'select id, state, epoch from runs'` is a
//! different debugging experience from writing a program to find out.
//!
//! **This API is synchronous.** SQLite's is, and pretending otherwise by sprinkling
//! `async` over blocking calls would hide the cost rather than remove it. Callers on an
//! async task must go through `tokio::task::spawn_blocking`; the operations here are
//! sub-millisecond on a local WAL database, but "usually fast" is not the same as
//! "safe to block a runtime thread on".

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod blobs;
pub mod deliveries;
pub mod observations;
pub mod rules;
pub mod runs;
pub mod schedules;
pub mod schema;

use offload_core::BlobHash;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io error on {path}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("blob {0} is not stored here")]
    MissingBlob(String),
    #[error("blob {expected} is corrupt — its contents hash to {actual}")]
    CorruptBlob { expected: String, actual: String },
    #[error("could not encode state: {0}")]
    Encode(String),
    #[error("stored state for {what} could not be decoded: {reason}")]
    Decode { what: String, reason: String },
    // The three below quote the needle at each construction site rather than here, because
    // half of them add a clause after it — and a message that put the backticks around the
    // whole string read as if `01a0… (ambiguous — 01a0…790a or 01a0…744b)` were the thing
    // somebody had typed. What is inside the quotes has to be what they typed, especially now
    // that what follows is an id they are meant to copy.
    #[error("no run matching {0}")]
    NoSuchRun(String),
    /// Nothing was typed where an identifier was expected.
    ///
    /// Its own variant rather than a `NoSuchRun` with an empty needle in it, because the two are
    /// different sentences: one says *there is no such run* and this says *you did not name
    /// one*. Quoting the empty string produced ``no run matching `` ``, which reads as a lookup
    /// that failed and sends somebody to check their ids. An unset shell variable is how this
    /// arrives, and saying so is the whole of the help.
    #[error("no run id was given — `offload ps` lists them")]
    NoRunGiven,
    #[error("no rule matching {0}")]
    NoSuchRule(String),
    #[error("no schedule matching {0}")]
    NoSuchSchedule(String),
}

/// Milliseconds since the unix epoch.
///
/// The store is the one place a clock is legitimate: rows carry timestamps, and `now` has
/// to come from somewhere. Domain logic still takes time as a parameter.
#[must_use]
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A node's durable state.
///
/// Cheap to clone: the connection is shared behind a mutex. One daemon writes, so
/// contention is not a concern; the mutex is there because `rusqlite::Connection` is
/// `Send` but not `Sync`.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    root: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("root", &self.root).finish()
    }
}

impl Store {
    /// Open (creating if needed) the state database under `root`.
    pub fn open(root: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(root).map_err(|e| StoreError::Io {
            path: root.to_path_buf(),
            reason: e.to_string(),
        })?;

        let conn = Connection::open(root.join("state.db"))?;
        schema::configure(&conn)?;
        schema::migrate(&conn)?;

        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
            root: root.to_path_buf(),
        })
    }

    /// In-memory store, for tests.
    ///
    /// **The blob root is per-store**, which the one-directory-for-everybody version it
    /// replaces was not. A blob lives on disk rather than in the connection, so every
    /// in-memory store in the process shared one directory — and it outlived the process, so a
    /// blob written by one test was visible to a different test in a later run. Two
    /// consequences, and the second is the one that cost a session: `has_blob` was not a
    /// question about *this* node, so **"a node that does not have this checkpoint's blobs"
    /// could not be staged at all** — which is precisely the migration `resume` failed on, and
    /// precisely why nothing in 900 tests noticed. The uniqueness is the pid and a counter;
    /// nothing here removes the directory, exactly as before.
    pub fn open_memory() -> Result<Self, StoreError> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let conn = Connection::open_in_memory()?;
        schema::configure(&conn)?;
        schema::migrate(&conn)?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
            root: std::env::temp_dir()
                .join("offload-memory-store")
                .join(format!("{}-{n}", std::process::id())),
        })
    }

    /// Recover from a poisoned lock rather than propagating a panic: one failed query
    /// should not take the daemon's whole state with it.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Direct connection access, for queries this module does not wrap.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.lock()
    }

    #[must_use]
    pub fn blob_root(&self) -> PathBuf {
        self.root.join("blobs")
    }

    // -- blobs ------------------------------------------------------------------

    pub fn put_blob(&self, bytes: &[u8]) -> Result<BlobHash, StoreError> {
        blobs::put(&self.lock(), &self.blob_root(), bytes)
    }

    pub fn get_blob(&self, hash: BlobHash) -> Result<Vec<u8>, StoreError> {
        blobs::get(&self.lock(), &self.blob_root(), hash)
    }

    #[must_use]
    pub fn has_blob(&self, hash: BlobHash) -> bool {
        blobs::has(&self.blob_root(), hash)
    }

    /// How big a blob this node holds is. `None` when it does not hold it.
    #[must_use]
    pub fn blob_size(&self, hash: BlobHash) -> Option<u64> {
        blobs::size(&self.blob_root(), hash)
    }

    pub fn blob_count(&self) -> Result<u64, StoreError> {
        let count: i64 = self
            .lock()
            .query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    pub fn blob_bytes(&self) -> Result<u64, StoreError> {
        blobs::total_size(&self.lock())
    }

    pub fn collect_garbage(&self, older_than_ms: u64) -> Result<usize, StoreError> {
        blobs::collect_garbage(&self.lock(), &self.blob_root(), older_than_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_twice_reuses_the_same_database() {
        // The property phase 1 lacked: a restart must find its state, not start over.
        let dir = std::env::temp_dir().join(format!("offload-store-open-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();

        let first = Store::open(&dir).expect("open");
        let hash = first.put_blob(b"survives a restart").expect("put");
        drop(first);

        let second = Store::open(&dir).expect("reopen");
        assert_eq!(second.get_blob(hash).expect("get"), b"survives a restart");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_database_is_readable_by_the_sqlite_cli() {
        // ADR-0009's whole argument. If this file needs our code to be understood, the
        // reason for choosing SQLite over redb evaporated.
        let dir = std::env::temp_dir().join(format!("offload-store-cli-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let store = Store::open(&dir).expect("open");
        store.put_blob(b"x").expect("put");
        drop(store);

        let db = dir.join("state.db");
        assert!(db.is_file(), "state.db exists at a predictable path");

        // SQLite's file magic, so any tool can open it.
        let header = std::fs::read(&db).expect("read");
        assert!(
            header.starts_with(b"SQLite format 3\0"),
            "not a SQLite file"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
