//! Naming things on disk: the branch a run works on, and what counts as a repository.
//!
//! [`RepoSource`] itself lives in `offload-core` — placement has to reason about portability
//! before anything is cloned — and is re-exported here, where every caller expects it.

pub use offload_core::repo::{Portability, RepoReach, RepoSource};

use std::path::Path;

/// The branch an agent works on for a given run.
///
/// Derived from the run id and therefore identical on every node — a migrated run
/// re-creates the same branch name, which is what makes the git bundle from the old holder
/// apply cleanly on the new one.
///
/// Uses the **full** run id, not the abbreviated one. Run ids are UUIDv7, whose leading
/// bytes are a millisecond timestamp: the first four bytes are identical for every run
/// submitted within about 65 seconds of each other. An abbreviated branch name therefore
/// collides constantly, and git refuses the second worktree with "already used by
/// worktree" — which is how this was found, by submitting two runs a minute apart.
#[must_use]
pub fn run_branch(run: offload_core::RunId) -> String {
    format!("offload/run-{run}")
}

/// Does `path` look like a git repository?
#[must_use]
pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists() || path.join("HEAD").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::RunId;

    #[test]
    fn same_basename_different_repo_gets_a_different_cache_key() {
        // The bug this prevents: two unrelated repos sharing one mirror, so a run on one
        // silently sees the other's objects.
        let a = RepoSource::parse("/work/team-a/api").cache_key();
        let b = RepoSource::parse("/work/team-b/api").cache_key();
        assert_ne!(a, b);
        assert!(a.starts_with("api-"), "{a}");
        assert!(b.starts_with("api-"), "{b}");
    }

    #[test]
    fn cache_keys_are_stable_across_runs_and_releases() {
        // Hard-coded because this value names a directory that must still be found after
        // an upgrade. If this test fails, every node silently re-clones every repo.
        assert_eq!(
            RepoSource::parse("https://github.com/me/api.git").cache_key(),
            "api-c2bb134d497a1b78"
        );
    }

    #[test]
    fn cache_keys_are_filesystem_safe() {
        for repo in [
            "git@github.com:me/api.git",
            "https://example.com/a b/c?d=e",
            "/",
            "",
        ] {
            let key = RepoSource::parse(repo).cache_key();
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "unsafe key {key:?} from {repo:?}"
            );
            assert!(!key.is_empty());
        }
    }

    #[test]
    fn branch_names_are_derived_not_random() {
        // A migrated run must re-create the same branch on the receiving node, or the
        // bundle from the old holder has nothing to apply onto.
        let run = RunId::from_bytes([0xab; 16]);
        assert_eq!(run_branch(run), format!("offload/run-{run}"));
        assert_eq!(run_branch(run), run_branch(run));
    }

    #[test]
    fn runs_sharing_a_timestamp_prefix_still_get_distinct_branches() {
        // Run ids are UUIDv7: the leading bytes are a millisecond clock, so every run
        // submitted within ~65 seconds shares its first four bytes. Naming branches from
        // the abbreviated id made the second run fail with "already used by worktree".
        let mut a = [0u8; 16];
        let mut b = [0u8; 16];
        a[..6].copy_from_slice(&[0x01, 0x9f, 0x9a, 0x2b, 0x00, 0x11]);
        b[..6].copy_from_slice(&[0x01, 0x9f, 0x9a, 0x2b, 0x00, 0x11]);
        a[15] = 0xaa;
        b[15] = 0xbb;

        let (a, b) = (RunId::from_bytes(a), RunId::from_bytes(b));
        assert_eq!(
            a.short(),
            b.short(),
            "the abbreviated ids really do collide"
        );
        assert_ne!(run_branch(a), run_branch(b), "but their branches must not");
    }
}
