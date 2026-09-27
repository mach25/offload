//! Reading what the agent has actually done to a worktree.
//!
//! Two consumers, both important. `offload ps` wants a one-line summary. Phase 3's
//! checkpoint wants to know precisely which files must travel — and per ADR-0003 the
//! uncommitted work is the fragile, valuable part, so getting this wrong loses exactly
//! what the user cared about.

/// What changed in a worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    /// Tracked files with staged or unstaged modifications.
    pub modified: Vec<String>,
    /// Files git does not know about and is not ignoring. These matter: a newly written
    /// source file is untracked, and dropping it on migration silently discards work.
    pub untracked: Vec<String>,
    pub deleted: Vec<String>,
    /// Commits the agent made on the run branch, ahead of where it started.
    pub commits_ahead: u32,
}

impl WorktreeStatus {
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.modified.is_empty() || !self.untracked.is_empty() || !self.deleted.is_empty()
    }

    /// Is there anything here worth carrying to another node?
    #[must_use]
    pub fn has_work(&self) -> bool {
        self.is_dirty() || self.commits_ahead > 0
    }

    #[must_use]
    pub fn changed_files(&self) -> usize {
        self.modified.len() + self.untracked.len() + self.deleted.len()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        if !self.has_work() {
            return "clean".to_string();
        }
        let mut parts = Vec::new();
        if self.commits_ahead > 0 {
            parts.push(format!("{} commit(s)", self.commits_ahead));
        }
        if !self.modified.is_empty() {
            parts.push(format!("{} modified", self.modified.len()));
        }
        if !self.untracked.is_empty() {
            parts.push(format!("{} new", self.untracked.len()));
        }
        if !self.deleted.is_empty() {
            parts.push(format!("{} deleted", self.deleted.len()));
        }
        parts.join(", ")
    }
}

/// Parse `git status --porcelain=v1 --untracked-files=all -z`.
///
/// NUL-separated because filenames may contain spaces, quotes, or newlines, and the
/// newline-delimited form quotes and escapes them in a way that is easy to mis-unescape.
/// A path we fail to parse is a file we fail to migrate.
#[must_use]
pub fn parse_porcelain(raw: &str) -> WorktreeStatus {
    let mut status = WorktreeStatus::default();
    let mut entries = raw.split('\0').filter(|e| !e.is_empty()).peekable();

    while let Some(entry) = entries.next() {
        // Format: XY<space>PATH, where X is the index state and Y the worktree state.
        if entry.len() < 3 {
            continue;
        }
        let code: Vec<char> = entry.chars().take(2).collect();
        let path = entry[3..].to_string();
        let (index, worktree) = (code[0], code[1]);

        match (index, worktree) {
            ('?', '?') => status.untracked.push(path),
            // Renames carry a second NUL-separated path (the origin); consume it so it
            // isn't parsed as its own entry.
            ('R', _) | (_, 'R') => {
                entries.next();
                status.modified.push(path);
            }
            ('D', _) | (_, 'D') => status.deleted.push(path),
            ('!', '!') => {} // ignored
            _ => status.modified.push(path),
        }
    }

    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_common_states() {
        let raw = " M src/main.rs\0?? notes.md\0 D old.rs\0A  added.rs\0";
        let s = parse_porcelain(raw);
        assert_eq!(s.modified, vec!["src/main.rs", "added.rs"]);
        assert_eq!(s.untracked, vec!["notes.md"]);
        assert_eq!(s.deleted, vec!["old.rs"]);
        assert!(s.is_dirty());
    }

    #[test]
    fn handles_paths_with_spaces() {
        // The reason for -z. Newline-delimited porcelain would quote this, and a
        // mis-unescaped path is a file that silently doesn't migrate.
        let s = parse_porcelain(" M src/my file.rs\0?? a b c.txt\0");
        assert_eq!(s.modified, vec!["src/my file.rs"]);
        assert_eq!(s.untracked, vec!["a b c.txt"]);
    }

    #[test]
    fn a_rename_consumes_its_origin_path() {
        // Renames emit two NUL-separated paths. Failing to consume the second turns it
        // into a phantom entry.
        let s = parse_porcelain("R  new.rs\0old.rs\0?? real-new.txt\0");
        assert_eq!(s.modified, vec!["new.rs"]);
        assert_eq!(s.untracked, vec!["real-new.txt"]);
        assert_eq!(s.changed_files(), 2);
    }

    #[test]
    fn a_clean_tree_reports_clean() {
        let s = parse_porcelain("");
        assert!(!s.is_dirty());
        assert!(!s.has_work());
        assert_eq!(s.summary(), "clean");
    }

    #[test]
    fn committed_work_counts_even_when_the_tree_is_clean() {
        // A run that committed everything still has work worth migrating.
        let mut s = parse_porcelain("");
        s.commits_ahead = 3;
        assert!(!s.is_dirty());
        assert!(s.has_work());
        assert_eq!(s.summary(), "3 commit(s)");
    }

    #[test]
    fn summary_reads_like_a_sentence() {
        let mut s = parse_porcelain(" M a.rs\0?? b.rs\0");
        s.commits_ahead = 2;
        assert_eq!(s.summary(), "2 commit(s), 1 modified, 1 new");
    }
}
