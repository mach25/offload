//! Agent processes a previous incarnation of this daemon left behind.
//!
//! An agent runs in its own process group and the only handle on it is the `RunHandle`, which
//! lives in memory — exactly as durable as the daemon. So a daemon that is **killed** rather
//! than shut down leaves its agent running. Measured, on a two-node fleet: `kill -9` on
//! `offloadd` four turns into a run, and the agent went on to finish all twenty-one files of its
//! task over the following minute, writing into a worktree the fleet had already moved the run
//! out of. The last file landed sixty-two seconds after the daemon died.
//!
//! **Nothing in the product could stop it.** `offload cancel` resolves a run to its *holder*,
//! which by then is the node that took the run over, so the cancel goes to the wrong machine —
//! and the right machine has no daemon listening. Fencing does not reach it either: an epoch
//! check refuses a *write to the record*, and this process is not writing to any record. What it
//! is still doing is spending money and using whatever tool grants, allowlists and resources the
//! run was given, on a machine nobody is watching.
//!
//! **And the daemon's own recovery message walks somebody into the worst version of it.** After a
//! restart `recover` marks the run `failed — resumable from turn N with offload resume <id>`, and
//! `resume` *adopts the worktree in place*. The guard against starting a second agent into one
//! worktree reads `self.live`, which is in memory and therefore empty in a daemon that has just
//! started — so it cannot see the agent the last incarnation left. Measured, with this sweep
//! disabled: daemon killed at turn 4, daemon restarted, `offload resume`, and two `claude`
//! processes with the same worktree as their cwd, both working. That is the double execution
//! CLAUDE.md calls the failure mode that matters, on one machine, in one checkout, needing no
//! fleet and no partition — reached by doing exactly what the daemon told the operator to do.
//!
//! That is worse than it first looks, because it is the ordinary case rather than the exotic
//! one. The daemon dying while the machine stays up is a crash, an OOM kill, a `kill -9`, a
//! package upgrade — and when the daemon comes back, `Supervisor::recover` walks past the run
//! entirely, because by then somebody else holds it and recovering a peer's run is exactly what
//! that function must not do. So the leftover is invisible for ever.
//!
//! So the pid is written down beside the run, and a starting daemon sweeps what the last one
//! left. Three deliberate shapes:
//!
//! * **Node-local, and not on the `Run`.** A pid means nothing on another machine, so gossiping
//!   one would be a field with no owner — the merge bug ADR-0005's table exists to prevent. It
//!   is a file in the state directory, the same shape `resource` uses for a run's granted MCP
//!   config and for the same reason.
//! * **A note still here at startup is stale, and that is guaranteed rather than assumed.**
//!   `statedir` locks the directory to one daemon, so there is no second live `offloadd` whose
//!   agents these could be. Without that lock this sweep would be a way for one node to kill
//!   another's work.
//! * **A pid is reused, so it is checked before it is signalled.** The note carries the agent's
//!   session id and the sweep requires `/proc/<pid>/cmdline` to still name it. Where that cannot
//!   be read — the process is gone, or this is not Linux — nothing is signalled, because the
//!   safe answer and the convenient answer differ and killing an unidentified pid is how a sweep
//!   for stray agents ends up stopping somebody's editor.

use offload_core::RunId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How long a leftover agent gets between being asked to stop and being killed.
///
/// Short: nothing is reading its output, so there is no turn to finish and nothing it can
/// usefully write down. This is politeness towards whatever it shelled out to, not patience.
const GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// What is written beside a running agent so a later daemon can recognise and stop it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Note {
    /// The agent's process group id, which is also its pid.
    pid: u32,
    /// The agent's own session id, as it appears in its command line. This is what makes the
    /// difference between stopping a leftover agent and stopping whatever now holds that pid.
    session: String,
}

fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("agents")
}

fn note_path(state_dir: &Path, run: RunId) -> PathBuf {
    dir(state_dir).join(format!("{run}.json"))
}

/// Write down the process group an agent is running in.
///
/// Best effort, and deliberately not fatal: failing to write the note must not stop a run that
/// is otherwise fine from starting. What it costs is the ability to reap this one agent if this
/// daemon is killed, which is worth a warning and not a refusal.
pub fn remember(state_dir: &Path, run: RunId, pid: u32, session: &str) {
    let path = note_path(state_dir, run);
    let note = Note {
        pid,
        session: session.to_string(),
    };
    let written = std::fs::create_dir_all(dir(state_dir))
        .and_then(|()| serde_json::to_vec(&note).map_err(std::io::Error::other))
        .and_then(|bytes| std::fs::write(&path, bytes));
    if let Err(e) = written {
        tracing::warn!(
            run_id = %run,
            path = %path.display(),
            error = %e,
            "could not note the agent's process group; a kill -9 of this daemon would leave it running"
        );
    }
}

/// Forget a run's agent, because this leg has ended and the process is gone.
///
/// Best effort for `forget_config`'s reason: there is nothing useful to do if it fails, and a
/// note left behind costs one identity check on the next start rather than a wrong kill.
pub fn forget(state_dir: &Path, run: RunId) {
    let path = note_path(state_dir, run);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = %e, "could not remove an agent note");
        }
    }
}

/// Stop every agent a previous incarnation of this daemon left running. Returns how many.
///
/// Called once at startup, before anything else takes work. A new daemon can never adopt an old
/// agent's stream — the pipe died with the process holding it — so an agent still running here
/// is producing turns nobody records, checkpoints nobody takes and output nobody reads, whatever
/// the fleet has since decided about the run.
///
/// **The lock is a parameter because it is the safety argument.** Everything above rests on "a
/// note still here at startup is stale", which is true only while one daemon can hold this
/// directory: run this without the lock and a second `offloadd` started on a state directory
/// already in use would kill the *first* one's agents on its way to failing — one node
/// destroying another's work, through the function written to stop exactly that. Ordering the
/// two calls correctly in `main` is a hope that survives until somebody moves a line; asking for
/// the guard makes it something the compiler checks.
pub fn sweep(state_dir: &Path, _held: &crate::statedir::StateDirLock) -> usize {
    let Ok(entries) = std::fs::read_dir(dir(state_dir)) else {
        return 0; // no notes have ever been written here
    };
    let mut stopped = 0;
    let mut pending = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(note) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Note>(&bytes).ok())
        else {
            // An unreadable note names no process, so there is nothing to signal and nothing
            // to be careful about. Drop it rather than reconsidering it on every start.
            let _ = std::fs::remove_file(&path);
            continue;
        };

        if !still_running(note.pid, &note.session) {
            let _ = std::fs::remove_file(&path);
            continue;
        }

        tracing::warn!(
            pid = note.pid,
            session = %note.session,
            "an agent outlived the daemon that spawned it; stopping it"
        );
        offload_agent::claude::terminate_process_group(note.pid);
        pending.push((path, note));
    }

    if pending.is_empty() {
        return 0;
    }
    std::thread::sleep(GRACE);
    for (path, note) in pending {
        if still_running(note.pid, &note.session) {
            offload_agent::claude::kill_process_group(note.pid);
        }
        let _ = std::fs::remove_file(&path);
        stopped += 1;
    }
    stopped
}

/// Is `pid` still the agent this note was written for?
///
/// Unknown is **not** yes. A pid whose command line cannot be read is one this process has no
/// business signalling: it may be gone, it may belong to somebody else, and the difference is
/// invisible from here.
fn still_running(pid: u32, session: &str) -> bool {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(cmdline) => names_session(&cmdline, session),
        Err(_) => false,
    }
}

/// Does this `/proc/<pid>/cmdline` belong to an agent running `session`?
///
/// The arguments are NUL-separated, and the session id appears as its own argument — after
/// `--session-id` on a fresh run and after `--resume` on a migrated one, which is why this looks
/// for the value rather than for either flag.
fn names_session(cmdline: &[u8], session: &str) -> bool {
    !session.is_empty()
        && cmdline
            .split(|b| *b == 0)
            .any(|arg| arg == session.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("offload-leftovers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    #[test]
    fn a_note_round_trips_and_is_forgotten() {
        let state = temp("roundtrip");
        let run = RunId::from_bytes([3; 16]);
        remember(&state, run, 4242, "sess-abc");

        let path = note_path(&state, run);
        assert!(path.is_file(), "the note should be on disk");
        let note: Note =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("parse");
        assert_eq!(note.pid, 4242);
        assert_eq!(note.session, "sess-abc");

        forget(&state, run);
        assert!(!path.exists());
        // And forgetting twice is not an error: a leg can end more than one way.
        forget(&state, run);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_pid_that_is_not_the_agent_any_more_is_not_signalled() {
        // The reuse hazard, which is the whole reason the note carries a session id. This
        // process is certainly alive and certainly not an agent, so a sweep that trusted the
        // pid alone would signal the test runner's own process group.
        let mine = std::process::id();
        assert!(!still_running(mine, "sess-that-is-not-here"));

        // And the identity check is on the *value*, because a fresh run spells it
        // `--session-id <uuid>` and a migrated one `--resume <uuid>`.
        let fresh = b"claude\0--session-id\0sess-1\0--print\0".as_slice();
        let resumed = b"claude\0--resume\0sess-1\0--fork-session\0".as_slice();
        assert!(names_session(fresh, "sess-1"));
        assert!(names_session(resumed, "sess-1"));
        assert!(!names_session(fresh, "sess-2"));
        // A note with no session names nothing, and must never match everything.
        assert!(!names_session(fresh, ""));
    }

    #[test]
    fn a_note_for_a_process_that_is_gone_is_just_removed() {
        // The ordinary case after a clean shutdown that raced: the agent is gone, so there is
        // nothing to stop, and the sweep should not report having stopped anything.
        let state = temp("gone");
        // A pid that cannot be running: the kernel's own maximum plus one.
        remember(&state, RunId::from_bytes([4; 16]), u32::MAX, "sess-gone");
        let held = crate::statedir::lock(&state).expect("lock");
        assert_eq!(sweep(&state, &held), 0);
        assert!(
            !note_path(&state, RunId::from_bytes([4; 16])).exists(),
            "the stale note should be cleaned up"
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_state_directory_with_no_notes_sweeps_nothing() {
        let state = temp("empty");
        let held = crate::statedir::lock(&state).expect("lock");
        assert_eq!(sweep(&state, &held), 0);
        let _ = std::fs::remove_dir_all(&state);
    }
}
