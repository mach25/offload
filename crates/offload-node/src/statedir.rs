//! One daemon per state directory, enforced rather than assumed.
//!
//! `server::bind` used to say that a live daemon "already holds the lock on the state dir", and
//! nothing did. What actually happened when two `offloadd` processes were pointed at one
//! directory was worse than a confusing error: the second unlinked the first's socket and bound
//! its own, so the first kept running, kept renewing leases and kept holding runs while every
//! `offload` command reached the second. Two daemons, one registry, and no symptom anywhere.
//!
//! Two guards, because the two ways in are different mistakes.
//!
//! **The lock** covers the state directory, which is what `OFFLOAD_STATE_DIR` means: mirrors,
//! worktrees, blobs, identity, `state.db`. It is a SQLite file held open in an exclusive
//! transaction, for [`crate::broker`]'s reasons and one more — the broker had already established
//! that this is the only genuinely multi-process lock available to a crate that forbids `unsafe`,
//! and having two mechanisms for "this file is mine" would be one too many.
//!
//! **The probe** covers the case the lock cannot see: a socket path pointed somewhere else by
//! config, so two daemons hold different state directories and share one socket. Before removing
//! what looks like a leftover, connect to it. Something that answers is not a leftover.
//!
//! Neither is a substitute for the other, and the lock is the one that fails *early* — before a
//! worktree is built or a lease is renewed, and with the path in the message.

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// The lock file's name inside the state directory.
///
/// Its own file rather than `state.db`, whose docs assert "one daemon writes" twice and which
/// the daemon needs to write to all day. A lock is held for the process's lifetime; a database
/// that is being written cannot be.
const LOCK_FILE: &str = "daemon.lock";

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error(
        "another offloadd is already using {path} — one daemon per state directory. \
         Point this one somewhere else with OFFLOAD_STATE_DIR, or stop the other."
    )]
    Held { path: PathBuf },
    #[error("could not take the lock on {path}: {reason}")]
    Unusable { path: PathBuf, reason: String },
}

/// This process's claim on a state directory. Held for as long as the daemon runs.
///
/// Dropping it releases the lock, which is why it is returned rather than leaked: the release
/// wants to happen when the daemon stops, including when it stops by unwinding. A process that
/// is killed releases it too, because the operating system closes the file — which is the
/// property a lock file written by hand does not have, and the reason a stale one is not a
/// state this has to recover from.
#[derive(Debug)]
pub struct StateDirLock {
    /// The open transaction *is* the lock. Never read from, and not dead code: this connection
    /// being alive is the whole mechanism.
    _held: Connection,
    path: PathBuf,
}

impl StateDirLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Claim a state directory for this process, or say who has it.
///
/// Fails fast rather than waiting: `busy_timeout` is zero, because "another daemon has this
/// directory" does not become untrue by being patient, and a daemon that blocked at startup
/// would look hung at exactly the moment somebody is watching it start.
pub fn lock(state_dir: &Path) -> Result<StateDirLock, LockError> {
    let path = state_dir.join(LOCK_FILE);
    if let Err(e) = std::fs::create_dir_all(state_dir) {
        return Err(LockError::Unusable {
            path,
            reason: e.to_string(),
        });
    }
    let unusable = |e: rusqlite::Error| LockError::Unusable {
        path: state_dir.join(LOCK_FILE),
        reason: e.to_string(),
    };
    let held = Connection::open(&path).map_err(unusable)?;
    held.busy_timeout(std::time::Duration::ZERO)
        .map_err(unusable)?;
    // `BEGIN EXCLUSIVE` takes the write lock immediately rather than on first write, which is
    // the difference between a lock and an intention. It is never committed.
    match held.execute_batch("BEGIN EXCLUSIVE") {
        Ok(()) => Ok(StateDirLock { _held: held, path }),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::DatabaseBusy
                || e.code == rusqlite::ErrorCode::DatabaseLocked =>
        {
            Err(LockError::Held {
                path: state_dir.to_path_buf(),
            })
        }
        Err(e) => Err(unusable(e)),
    }
}

/// Is something already listening on this socket?
///
/// The half the lock cannot answer. `bind` removes what looks like a leftover so that a crash
/// does not make every subsequent start fail with "address in use" — a confusing way to say "the
/// last run died". The removal is right and the assumption behind it was not: a socket file with
/// a daemon behind it is not a leftover, and unlinking it disconnects nobody. The old process
/// keeps its listener, keeps its runs, and stops being reachable.
///
/// A connect is the only way to tell the two apart, and it is cheap: the daemon accepts and the
/// connection is dropped without a request, which every serving loop already handles as a client
/// that changed its mind.
#[must_use]
pub fn something_is_listening(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// The file in a checkouts directory naming the node whose checkouts it holds (ADR-0074).
const CHECKOUTS_OWNER: &str = ".offload-node";

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error(
        "{dir} holds the checkouts of another node ({owner}) — two nodes sharing it would each \
         remove the other's as abandoned. Set `[workspace] dir` in this node's config to a \
         directory of its own."
    )]
    Other { dir: PathBuf, owner: String },
    #[error("could not use {dir} for run checkouts: {reason}")]
    Unusable { dir: PathBuf, reason: String },
}

/// Claim a checkouts directory for this node, or refuse because another node has (ADR-0074).
///
/// The checkout sweep reclaims any checkout in the directory that no run of *this* node's still
/// needs, which is right for a directory of its own and destructive in a shared one: two nodes
/// each read the other's live checkouts as abandoned. Under the state directory the lock above
/// already guarantees one daemon. Anywhere else — `~/offload`, shared by default — this marker
/// does, and it fails at start with the setting to change, before a checkout is built.
pub fn claim_checkouts(
    dir: &Path,
    state_dir: &Path,
    node: offload_core::NodeId,
) -> Result<(), ClaimError> {
    if dir.starts_with(state_dir) {
        return Ok(());
    }
    let unusable = |e: std::io::Error| ClaimError::Unusable {
        dir: dir.to_path_buf(),
        reason: e.to_string(),
    };
    std::fs::create_dir_all(dir).map_err(unusable)?;
    let marker = dir.join(CHECKOUTS_OWNER);
    let mine = node.to_string();
    // Created exclusively, so two daemons starting together cannot both believe they claimed it.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(mut file) => {
            use std::io::Write;
            return file.write_all(mine.as_bytes()).map_err(unusable);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(unusable(e)),
    }
    let owner = std::fs::read_to_string(&marker).map_err(unusable)?;
    if owner.trim() == mine {
        Ok(())
    } else {
        Err(ClaimError::Other {
            dir: dir.to_path_buf(),
            owner: owner.trim().chars().take(12).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    /// ADR-0074: a shared checkouts directory is claimed by the first node and refused to the
    /// next, since each node's sweep would remove the other's checkouts. The state directory's own
    /// needs no marker, because its lock already means one daemon.
    #[test]
    fn a_checkouts_directory_outside_the_state_dir_belongs_to_one_node() {
        let base = std::env::temp_dir().join(format!("offload-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state = base.join("state");
        let shared = base.join("offload");
        let a = offload_core::NodeId::from_bytes([1; 32]);
        let b = offload_core::NodeId::from_bytes([2; 32]);

        assert!(
            claim_checkouts(&shared, &state, a).is_ok(),
            "the first claims it"
        );
        assert!(
            claim_checkouts(&shared, &state, a).is_ok(),
            "and keeps it across restarts"
        );
        let refused = claim_checkouts(&shared, &state, b).expect_err("another node is refused");
        let said = refused.to_string();
        assert!(said.contains("[workspace] dir"), "{said}");
        assert!(said.contains("010101010101"), "names the owner: {said}");

        let inside = state.join("worktrees");
        assert!(
            claim_checkouts(&inside, &state, b).is_ok(),
            "the state dir's own is not claimed"
        );
        assert!(!inside.join(CHECKOUTS_OWNER).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    use super::*;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "offload-statedir-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("temp dir");
        base
    }

    #[test]
    fn a_second_daemon_is_told_which_directory_is_taken() {
        let dir = tempdir();
        let first = lock(&dir).expect("first claim");
        let err = lock(&dir).expect_err("a second daemon must not get in");
        assert!(matches!(err, LockError::Held { .. }));
        // The message has to name the directory: somebody meeting this is usually running two
        // nodes on one machine and has forgotten to move one of them.
        let message = err.to_string();
        assert!(message.contains(&dir.display().to_string()));
        assert!(
            message.contains("OFFLOAD_STATE_DIR"),
            "must name the way out"
        );

        // And it is released by the process going away, not by anything remembering to tidy up.
        drop(first);
        let _second = lock(&dir).expect("the directory is free again");
    }

    #[test]
    fn two_state_directories_do_not_contend() {
        // The arrangement `OFFLOAD_STATE_DIR` exists for, and the one ADR-0012 requires for a
        // device in two fleets. Locking anything wider than the directory would break it.
        let base = tempdir();
        let _a = lock(&base.join("alpha")).expect("alpha");
        let _b = lock(&base.join("bravo")).expect("bravo");
    }

    #[test]
    fn a_socket_nobody_is_behind_reads_as_a_leftover() {
        let dir = tempdir();
        let path = dir.join("offloadd.sock");
        assert!(
            !something_is_listening(&path),
            "nothing there at all is not somebody listening"
        );

        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        assert!(something_is_listening(&path), "a live listener answers");

        // A crash leaves the file with nothing behind it, which is the case `bind` removes.
        drop(listener);
        assert!(
            path.exists(),
            "the file outlives the listener — that is the trap"
        );
        assert!(!something_is_listening(&path));
    }
}
