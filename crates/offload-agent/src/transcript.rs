//! Locating an agent's session transcript on disk.
//!
//! This is the file that makes resume-mid-conversation possible, so finding it reliably is
//! load-bearing for migration (ADR-0003).
//!
//! Claude Code stores transcripts at
//! `<config dir>/projects/<cwd with '/' replaced by '-'>/<session-id>.jsonl`, where the config
//! dir is `~/.claude` unless `$CLAUDE_CONFIG_DIR` says otherwise ([`config_dir`]). Verified
//! against a real run — but **the path is derived from the working directory**, which has
//! a consequence that is easy to miss: a run migrating to another node must have its
//! transcript written to the path matching *that node's* worktree, not the one it came
//! from. Same session id, different location.
//!
//! Because the slug rule is an undocumented implementation detail of somebody else's tool,
//! [`find`] computes the expected path but falls back to searching for the session id.
//! Guessing wrong then reporting "no transcript" would silently turn a resumable run into
//! a restart-from-zero.

use std::path::{Path, PathBuf};

/// Where Claude Code keeps its state for this user.
///
/// `$CLAUDE_CONFIG_DIR` when the owner has moved it, `~/.claude` otherwise. **Not a detail**: the
/// variable is a supported way to relocate the agent's state, and a daemon that assumed `~/.claude`
/// on a machine where it had been moved finds no transcript, fails every capture, and leaves a run
/// that has been checkpointing nothing all night — loud in the run's log and invisible everywhere
/// else. Found exactly that way, on a machine that had it set.
///
/// Read from *this* process's environment on purpose. The agent is spawned as a child and inherits
/// it, so the two cannot disagree; asking the environment is how they stay in step rather than a
/// guess about somebody's setup.
///
/// `offload-probe` needs the same answer for `.credentials.json` and has its own copy, because a
/// probe that pulled in the whole agent adapter to resolve one path would be the dependency doing
/// more harm than the duplication. Both are three lines and both are tested; if a third appears,
/// that is the point at which this belongs somewhere shared.
#[must_use]
pub fn config_dir(home: &Path) -> PathBuf {
    resolve_config_dir(home, std::env::var_os("CLAUDE_CONFIG_DIR"))
}

/// The pure half of [`config_dir`], so the rule can be tested without touching the environment
/// of whatever else is running in this process.
fn resolve_config_dir(home: &Path, configured: Option<std::ffi::OsString>) -> PathBuf {
    match configured {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        // An empty variable is somebody unsetting it awkwardly, not a request to use the
        // current directory.
        _ => home.join(".claude"),
    }
}

/// Slugify a working directory into Claude Code's project-directory name.
///
/// `/` becomes `-`; the leading slash produces a leading `-`. Verified against a real
/// session directory. Other characters are passed through — the observed behaviour for
/// paths that already contain `-` is that they are preserved, producing the doubled `--`
/// seen when a path segment itself starts with a dash.
#[must_use]
pub fn project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy().replace('/', "-")
}

/// Where the transcript for `session_id` is expected to live.
///
/// Takes the agent's **config directory** rather than a home directory, because where that is
/// depends on the environment ([`config_dir`]) and a function that read the environment could not
/// be tested without changing it for everything else in the process.
#[must_use]
pub fn expected_path(config: &Path, cwd: &Path, session_id: &str) -> PathBuf {
    config
        .join("projects")
        .join(project_slug(cwd))
        .join(format!("{session_id}.jsonl"))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranscriptError {
    #[error("no transcript found for session {session_id}")]
    NotFound { session_id: String },
    #[error("could not read {path}: {reason}")]
    Unreadable { path: PathBuf, reason: String },
}

/// Find the transcript for a session.
///
/// Tries the computed path first, then scans the projects directory. The scan exists
/// because the slug rule is not ours to rely on: if the agent changes how it names project
/// directories, a computed-path-only lookup reports "no transcript" and the run silently
/// degrades from resumable to restart-from-scratch. The session id, by contrast, is
/// something we chose and passed in — it is the reliable key.
pub fn find(config: &Path, cwd: &Path, session_id: &str) -> Result<PathBuf, TranscriptError> {
    let expected = expected_path(config, cwd, session_id);
    if expected.is_file() {
        return Ok(expected);
    }

    let projects = config.join("projects");
    let filename = format!("{session_id}.jsonl");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return Err(TranscriptError::NotFound {
            session_id: session_id.to_string(),
        });
    };

    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            tracing::debug!(
                session_id,
                expected = %expected.display(),
                found = %candidate.display(),
                "transcript was not at the computed path; slug rule may have changed"
            );
            return Ok(candidate);
        }
    }

    Err(TranscriptError::NotFound {
        session_id: session_id.to_string(),
    })
}

/// Place a transcript where the agent will find it for `cwd`.
///
/// The migration counterpart of [`find`]: the receiving node writes the blob it fetched
/// into the location derived from *its own* worktree path.
pub fn install(
    config: &Path,
    cwd: &Path,
    session_id: &str,
    contents: &[u8],
) -> Result<PathBuf, TranscriptError> {
    let path = expected_path(config, cwd, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| TranscriptError::Unreadable {
            path: parent.to_path_buf(),
            reason: e.to_string(),
        })?;
    }
    std::fs::write(&path, contents).map_err(|e| TranscriptError::Unreadable {
        path: path.clone(),
        reason: e.to_string(),
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_a_real_observed_directory() {
        // Both sides captured from this machine's ~/.claude/projects.
        assert_eq!(
            project_slug(Path::new(
                "/home/owner/Development/projects/example-project"
            )),
            "-home-owner-Development-projects-example-project"
        );
        assert_eq!(
            project_slug(Path::new(
                "/tmp/claude-1000/-home-owner-Development-projects-example-project/b395803e/scratchpad"
            )),
            "-tmp-claude-1000--home-owner-Development-projects-example-project-b395803e-scratchpad"
        );
    }

    #[test]
    fn a_transcript_lives_under_the_config_directory() {
        let p = expected_path(Path::new("/home/j/.claude"), Path::new("/w/repo"), "sess-1");
        assert_eq!(
            p,
            PathBuf::from("/home/j/.claude/projects/-w-repo/sess-1.jsonl")
        );
    }

    #[test]
    fn a_moved_config_directory_is_where_the_transcripts_are() {
        // The bug this rule exists for: `$CLAUDE_CONFIG_DIR` is a supported way to move the
        // agent's state, and a daemon that assumed `~/.claude` found no transcript, failed every
        // capture, and left a run checkpointing nothing all night.
        assert_eq!(
            resolve_config_dir(Path::new("/home/j"), Some("/srv/agent-state".into())),
            PathBuf::from("/srv/agent-state")
        );
        // Unset, and the awkward way people unset things.
        assert_eq!(
            resolve_config_dir(Path::new("/home/j"), None),
            PathBuf::from("/home/j/.claude")
        );
        assert_eq!(
            resolve_config_dir(Path::new("/home/j"), Some(String::new().into())),
            PathBuf::from("/home/j/.claude")
        );
    }

    #[test]
    fn install_then_find_round_trips() {
        let tmp = std::env::temp_dir().join(format!("offload-transcript-{}", std::process::id()));
        // An explicit config directory, never one derived from the environment: a test that
        // resolved `$CLAUDE_CONFIG_DIR` would write into somebody's real agent state.
        let config = tmp.join("agent-state");
        let cwd = Path::new("/work/some-repo");
        std::fs::create_dir_all(&config).expect("create config dir");

        let written = install(&config, cwd, "sess-abc", b"{\"a\":1}\n").expect("install");
        assert!(written.is_file());
        assert_eq!(find(&config, cwd, "sess-abc").expect("find"), written);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn find_falls_back_to_scanning_when_the_slug_is_wrong() {
        // Guards the migration failure mode: if the agent's slug rule changes, a
        // computed-path-only lookup reports "no transcript" and the run silently loses
        // its conversation instead of resuming.
        let tmp = std::env::temp_dir().join(format!("offload-scan-{}", std::process::id()));
        let config = tmp.join("agent-state");
        let unexpected = config.join("projects").join("some_other_slug");
        std::fs::create_dir_all(&unexpected).expect("create dirs");
        std::fs::write(unexpected.join("sess-xyz.jsonl"), b"{}").expect("write");

        let found = find(&config, Path::new("/completely/different"), "sess-xyz").expect("find");
        assert!(found.ends_with("some_other_slug/sess-xyz.jsonl"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_missing_transcript_is_reported_not_guessed() {
        let config = std::env::temp_dir().join("offload-nonexistent-agent-state");
        assert_eq!(
            find(&config, Path::new("/w"), "nope"),
            Err(TranscriptError::NotFound {
                session_id: "nope".into()
            })
        );
    }
}
