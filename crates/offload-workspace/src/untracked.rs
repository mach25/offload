//! Which untracked files travel with a checkpoint.
//!
//! ADR-0003 calls uncommitted work "the fragile, valuable part", and untracked files are
//! the worst of it: a source file the agent just created is untracked, and `git diff` does
//! not know it exists. Drop it and you have lost precisely what the run was doing.
//!
//! But the opposite is worse in practice. Phase 1's demo — one agent run that executed
//! `cargo test` — left **50 untracked files** in `target/`, in a repo with no `.gitignore`
//! to lean on. Bundling those would dwarf the work they surround, and on a phone over
//! mobile data it would be actively harmful.
//!
//! So `--exclude-standard` (which honours `.gitignore`) is the first filter and not a
//! sufficient one. What follows is deliberately conservative in the *include* direction
//! and loud about everything it drops: silently discarding a file the user cared about is
//! the failure this whole module exists to avoid, so anything excluded is named and
//! attributed.
//!
//! And the second filter is a list of names, which is a guess about somebody else's
//! repository — so it loses to what that repository *says*. A directory git already tracks
//! content in is content, whatever it is called. Measured on the repositories on one
//! developer's laptop: `build/` holds hand-written Dockerfiles and shell scripts in an
//! ingress chart, WordPress ships 132 tracked files under `wp-includes/js/dist/`, and two
//! more track `vendor/` and `node_modules/` outright. Without that check, an agent asked to
//! add a stage to `build/Dockerfile` writes `build/Dockerfile.debug`, and the checkpoint
//! reports it as build output and leaves it behind.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Directory names that are never hand-written source **in a repository that does not track
/// them**.
///
/// Belt and braces on top of `.gitignore`, because the repo that most needs this is the one
/// that has not got around to writing one — which was exactly phase 1's demo. Matched as a
/// path component, so `target/debug/x` is caught wherever it sits, and `src/target/mod.rs`
/// is safe as long as `src/target/` is a directory this repo keeps source in — which is the
/// only circumstance in which it is a module rather than an artefact.
const NEVER_SOURCE: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".gradle",
    ".next",
    ".nuxt",
    ".parcel-cache",
    ".turbo",
    ".stack-work",
    "DerivedData",
    ".terraform",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntrackedPolicy {
    /// Skip any single file larger than this. A big untracked file is almost never
    /// hand-written source; it is a binary, a log, or a downloaded artefact.
    pub max_file_bytes: u64,
    /// Stop once the selection reaches this size. A cap that is hit is reported, never
    /// silently applied.
    pub max_total_bytes: u64,
    /// Extra directory names to treat as build output, on top of the built-ins.
    pub also_never_source: Vec<String>,
}

impl Default for UntrackedPolicy {
    fn default() -> Self {
        UntrackedPolicy {
            // Generous for source, tight for artefacts. A 1 MiB hand-written file exists
            // but is rare enough that reporting it beats shipping every build output.
            max_file_bytes: 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
            also_never_source: Vec::new(),
        }
    }
}

/// Why a file did not travel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum Excluded {
    /// Lives under a directory that only ever holds build output.
    BuildOutput {
        directory: String,
    },
    TooLarge {
        size_bytes: u64,
        limit: u64,
    },
    /// The selection had already reached `max_total_bytes`.
    BudgetExhausted,
}

impl std::fmt::Display for Excluded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Excluded::BuildOutput { directory } => write!(f, "build output ({directory}/)"),
            Excluded::TooLarge { size_bytes, limit } => {
                write!(f, "{size_bytes} bytes, over the {limit} byte limit")
            }
            Excluded::BudgetExhausted => f.write_str("checkpoint size budget exhausted"),
        }
    }
}

/// What the policy decided.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub included: Vec<String>,
    pub excluded: Vec<(String, Excluded)>,
    pub included_bytes: u64,
}

impl Selection {
    /// A one-line account for the run's event log.
    ///
    /// Always says something when files were dropped. "Your new file did not migrate" has
    /// to be discoverable at the moment it happens, not inferred later from its absence.
    ///
    /// One count per reason, and not because three numbers read better than two: this line
    /// is the whole of what the operator is told, and the three reasons are fixed by three
    /// different things. A file left behind because the *checkpoint's* budget was full is
    /// not a file that was too big — `max_untracked_total_bytes` is the setting, not
    /// `max_untracked_file_bytes` — and it used to be reported as one, so the one action the
    /// message existed to prompt was the wrong one.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.excluded.is_empty() {
            return format!("{} untracked file(s)", self.included.len());
        }
        let count = |wanted: fn(&Excluded) -> bool| {
            self.excluded.iter().filter(|(_, why)| wanted(why)).count()
        };
        let build = count(|why| matches!(why, Excluded::BuildOutput { .. }));
        let large = count(|why| matches!(why, Excluded::TooLarge { .. }));
        let budget = count(|why| matches!(why, Excluded::BudgetExhausted));

        let mut parts = vec![format!("{} untracked file(s)", self.included.len())];
        if build > 0 {
            parts.push(format!("{build} skipped as build output"));
        }
        if large > 0 {
            parts.push(format!("{large} skipped as too large"));
        }
        if budget > 0 {
            parts.push(format!("{budget} skipped once the size budget was full"));
        }
        parts.join(", ")
    }
}

/// Decide which untracked files travel.
///
/// `entries` are `(path, size_bytes)` relative to the worktree root, already filtered by
/// git's own ignore rules. `tracked_dirs` are the directories that repository keeps tracked
/// files in, at any depth, as `/`-joined paths with no trailing slash — the repo's own answer
/// to what is content, which beats a list of names guessed from outside it. Pure: the
/// interesting cases are unit tests rather than a repo someone has to construct.
#[must_use]
pub fn select(
    entries: &[(String, u64)],
    policy: &UntrackedPolicy,
    tracked_dirs: &BTreeSet<String>,
) -> Selection {
    // Sort by size ascending, so a budget that runs out sacrifices the biggest files
    // rather than whichever happened to come first. Small files are overwhelmingly the
    // hand-written ones.
    let mut sorted: Vec<&(String, u64)> = entries.iter().collect();
    sorted.sort_by_key(|(path, size)| (*size, path.clone()));

    let mut selection = Selection::default();

    for (path, size) in sorted {
        if let Some(directory) = build_output_dir(path, policy, tracked_dirs) {
            selection
                .excluded
                .push((path.clone(), Excluded::BuildOutput { directory }));
            continue;
        }
        if *size > policy.max_file_bytes {
            selection.excluded.push((
                path.clone(),
                Excluded::TooLarge {
                    size_bytes: *size,
                    limit: policy.max_file_bytes,
                },
            ));
            continue;
        }
        if selection.included_bytes.saturating_add(*size) > policy.max_total_bytes {
            selection
                .excluded
                .push((path.clone(), Excluded::BudgetExhausted));
            continue;
        }
        selection.included_bytes += size;
        selection.included.push(path.clone());
    }

    selection.included.sort();
    selection.excluded.sort_by(|a, b| a.0.cmp(&b.0));
    selection
}

/// The build-output directory this path sits under, if any.
///
/// A matching name is only build output while the repository keeps nothing tracked there:
/// `target/` in this repo is an artefact directory, and `build/` in an ingress chart is
/// four hand-written files. Both are decided by the same question, asked of git rather than
/// guessed from the name — and asked of *that* directory, so a `target/` inside a tracked
/// `build/` is still caught.
fn build_output_dir(
    path: &str,
    policy: &UntrackedPolicy,
    tracked_dirs: &BTreeSet<String>,
) -> Option<String> {
    let mut components: Vec<&str> = path.split('/').collect();
    // Drop the final component: it is the file itself, and a *file* named `build` is
    // source. Only directories are build output.
    components.pop();

    let mut prefix = String::new();
    for component in components {
        if component.is_empty() {
            continue;
        }
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component);

        let never_source = NEVER_SOURCE.contains(&component)
            || policy
                .also_never_source
                .iter()
                .any(|extra| extra == component);
        if never_source && !tracked_dirs.contains(&prefix) {
            return Some(component.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(items: &[(&str, u64)]) -> Vec<(String, u64)> {
        items.iter().map(|(p, s)| ((*p).to_string(), *s)).collect()
    }

    /// A repository that tracks nothing — every one of these cases except the ones about
    /// tracked content, and the state phase 1's demo repo was in.
    fn untracked_repo() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn tracks(dirs: &[&str]) -> BTreeSet<String> {
        dirs.iter().map(|d| (*d).to_string()).collect()
    }

    #[test]
    fn a_new_source_file_travels() {
        // The case that matters most: the agent wrote this and nothing else knows it
        // exists.
        let selection = select(
            &entries(&[("src/new_module.rs", 2_000)]),
            &UntrackedPolicy::default(),
            &untracked_repo(),
        );
        assert_eq!(selection.included, vec!["src/new_module.rs"]);
        assert!(selection.excluded.is_empty());
    }

    #[test]
    fn build_output_does_not() {
        // Phase 1's demo, exactly: one `cargo test` left 50 untracked files under target/
        // in a repo with no .gitignore.
        let mut items = vec![("src/lib.rs".to_string(), 500u64)];
        for i in 0..50 {
            items.push((format!("target/debug/build/artifact-{i}.o"), 100_000));
        }

        let selection = select(&items, &UntrackedPolicy::default(), &untracked_repo());
        assert_eq!(selection.included, vec!["src/lib.rs"]);
        assert_eq!(selection.excluded.len(), 50);
        assert!(selection
            .excluded
            .iter()
            .all(|(_, e)| matches!(e, Excluded::BuildOutput { .. })));
    }

    #[test]
    fn a_file_named_like_a_build_directory_is_still_source() {
        // `build` as the last component is a file, not a directory.
        let selection = select(
            &entries(&[("scripts/build", 100), ("src/target", 100)]),
            &UntrackedPolicy::default(),
            &untracked_repo(),
        );
        assert_eq!(selection.included.len(), 2, "{selection:?}");
    }

    #[test]
    fn a_nested_build_directory_is_caught() {
        let selection = select(
            &entries(&[("crates/api/target/debug/x", 10)]),
            &UntrackedPolicy::default(),
            &untracked_repo(),
        );
        assert_eq!(
            selection.excluded[0].1,
            Excluded::BuildOutput {
                directory: "target".into()
            }
        );
    }

    #[test]
    fn oversized_files_are_reported_not_silently_dropped() {
        let policy = UntrackedPolicy {
            max_file_bytes: 1_000,
            ..UntrackedPolicy::default()
        };
        let selection = select(
            &entries(&[("big.bin", 5_000), ("small.rs", 100)]),
            &policy,
            &untracked_repo(),
        );

        assert_eq!(selection.included, vec!["small.rs"]);
        assert_eq!(
            selection.excluded,
            vec![(
                "big.bin".to_string(),
                Excluded::TooLarge {
                    size_bytes: 5_000,
                    limit: 1_000
                }
            )]
        );
    }

    #[test]
    fn the_budget_sacrifices_the_largest_files_first() {
        // If something has to go, it should be the artefact rather than the source file
        // that happened to be listed last.
        let policy = UntrackedPolicy {
            max_total_bytes: 1_000,
            ..UntrackedPolicy::default()
        };
        let selection = select(
            &entries(&[("huge.txt", 900), ("a.rs", 100), ("b.rs", 100)]),
            &policy,
            &untracked_repo(),
        );

        assert_eq!(selection.included, vec!["a.rs", "b.rs"]);
        assert_eq!(selection.excluded[0].0, "huge.txt");
        assert_eq!(selection.excluded[0].1, Excluded::BudgetExhausted);
    }

    #[test]
    fn exclusions_are_always_visible_in_the_summary() {
        // "Your new file did not migrate" has to be discoverable when it happens.
        let selection = select(
            &entries(&[("src/a.rs", 10), ("target/x", 10), ("huge", 99_999_999)]),
            &UntrackedPolicy::default(),
            &untracked_repo(),
        );
        let summary = selection.summary();
        assert!(summary.contains("1 untracked file"), "{summary}");
        assert!(summary.contains("build output"), "{summary}");
        assert!(summary.contains("too large"), "{summary}");
    }

    #[test]
    fn a_full_budget_is_not_reported_as_a_file_that_was_too_big() {
        // The two are fixed by different settings, and this line is the whole of what the
        // operator is told. `ordinary.rs` is well under the per-file limit; what stopped it
        // was the checkpoint's total.
        let policy = UntrackedPolicy {
            max_total_bytes: 100,
            ..UntrackedPolicy::default()
        };
        let selection = select(
            &entries(&[("a.rs", 100), ("ordinary.rs", 200)]),
            &policy,
            &untracked_repo(),
        );
        let summary = selection.summary();

        assert_eq!(selection.excluded[0].1, Excluded::BudgetExhausted);
        assert!(summary.contains("budget"), "{summary}");
        assert!(!summary.contains("too large"), "{summary}");
    }

    #[test]
    fn a_clean_selection_reads_plainly() {
        assert_eq!(
            select(
                &entries(&[("a.rs", 1)]),
                &UntrackedPolicy::default(),
                &untracked_repo()
            )
            .summary(),
            "1 untracked file(s)"
        );
        assert_eq!(
            select(&[], &UntrackedPolicy::default(), &untracked_repo()).summary(),
            "0 untracked file(s)"
        );
    }

    #[test]
    fn a_directory_the_repository_tracks_is_content_whatever_it_is_called() {
        // Measured, not imagined. An ingress chart on this laptop keeps four hand-written
        // files in `build/`, and WordPress ships 132 tracked files under
        // `wp-includes/js/dist/`. An agent asked to add a stage to `build/Dockerfile` writes
        // `build/Dockerfile.debug`, and a list of names would leave it behind.
        let selection = select(
            &entries(&[
                ("build/Dockerfile.debug", 400),
                ("wp-includes/js/dist/new-block.js", 900),
            ]),
            &UntrackedPolicy::default(),
            &tracks(&[
                "build",
                "wp-includes",
                "wp-includes/js",
                "wp-includes/js/dist",
            ]),
        );
        assert_eq!(
            selection.included,
            vec!["build/Dockerfile.debug", "wp-includes/js/dist/new-block.js"],
            "{selection:?}"
        );
    }

    #[test]
    fn a_module_directory_named_like_an_artefact_one_travels() {
        // What `NEVER_SOURCE` has claimed since it was written — `src/target/mod.rs` is
        // safe — which was false for as long as the rule was a list of names. It is true
        // here for the reason it should have been: this repo keeps source in `src/target`.
        let selection = select(
            &entries(&[("src/target/mod.rs", 300)]),
            &UntrackedPolicy::default(),
            &tracks(&["src", "src/target"]),
        );
        assert_eq!(selection.included, vec!["src/target/mod.rs"]);
    }

    #[test]
    fn an_artefact_directory_inside_a_tracked_one_is_still_caught() {
        // The question is asked of each directory in turn, so a repo that tracks `build/`
        // has not thereby signed for whatever a compiler dropped in `build/target/`.
        let selection = select(
            &entries(&[("build/target/debug/x.o", 10_000)]),
            &UntrackedPolicy::default(),
            &tracks(&["build"]),
        );
        assert_eq!(
            selection.excluded[0].1,
            Excluded::BuildOutput {
                directory: "target".into()
            }
        );
    }

    #[test]
    fn repos_can_name_their_own_build_directories() {
        let policy = UntrackedPolicy {
            also_never_source: vec!["_generated".to_string()],
            ..UntrackedPolicy::default()
        };
        let selection = select(
            &entries(&[("_generated/api.rs", 10)]),
            &policy,
            &untracked_repo(),
        );
        assert_eq!(selection.included.len(), 0);
        assert_eq!(
            selection.excluded[0].1,
            Excluded::BuildOutput {
                directory: "_generated".into()
            }
        );
    }
}
