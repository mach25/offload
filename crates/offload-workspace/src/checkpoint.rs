//! Capturing a worktree into something another machine can rebuild, and rebuilding it.
//!
//! Two artefacts, both produced by git so that restoring is git's problem rather than
//! ours (ADR-0003):
//!
//! * a **bundle** of the commits the agent made on its run branch, and
//! * a **patch** of everything not yet committed — modified tracked files *and* the
//!   untracked files the policy decided were worth carrying.
//!
//! Getting untracked files into a `git diff` needs `git add -N` (intent-to-add) first,
//! which mutates the index. That is safe here for a specific reason: checkpoints happen at
//! turn boundaries (ADR-0004), so the agent is between turns and not running a tool. The
//! intent-to-add is reverted immediately afterwards, so the agent's next `git status`
//! looks exactly as it did.

use crate::untracked::{select, Selection, UntrackedPolicy};
use crate::{git, git_bytes, Workspace, WorkspaceError};
use std::collections::BTreeSet;
use std::path::Path;

/// Everything needed to rebuild a worktree elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// Git bundle of `base..branch`. `None` when the agent committed nothing.
    pub bundle: Option<Vec<u8>>,
    /// Patch of uncommitted work, including selected untracked files. `None` when clean.
    pub patch: Option<Vec<u8>>,
    /// What the untracked-file policy decided, so the run's log can say what it dropped.
    pub untracked: Selection,
    pub base_commit: String,
    pub branch: String,
}

impl Capture {
    /// Is there anything here worth moving?
    #[must_use]
    pub fn has_work(&self) -> bool {
        self.bundle.is_some() || self.patch.is_some()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(bundle) = &self.bundle {
            parts.push(format!("{} byte bundle", bundle.len()));
        }
        if let Some(patch) = &self.patch {
            parts.push(format!("{} byte patch", patch.len()));
        }
        if parts.is_empty() {
            parts.push("nothing uncommitted".to_string());
        }
        format!("{} ({})", parts.join(" + "), self.untracked.summary())
    }
}

/// Capture a worktree's work.
///
/// Must be called at a turn boundary — see the module note on why the index mutation is
/// safe there and nowhere else.
pub async fn capture(
    workspace: &Workspace,
    policy: &UntrackedPolicy,
) -> Result<Capture, WorkspaceError> {
    let path = &workspace.path;

    // Commits the agent made, if any. `rev-list --count` rather than trusting the branch
    // to differ: bundling an empty range is an error, not an empty bundle.
    let range = format!("{}..HEAD", workspace.base_commit);
    let ahead: u32 = git(Some(path), &["rev-list", "--count", &range])
        .await?
        .trim()
        .parse()
        .unwrap_or(0);

    let bundle = if ahead > 0 {
        // `git bundle create -` writes to stdout; capture it as bytes.
        Some(git_bytes(Some(path), &["bundle", "create", "-", &range]).await?)
    } else {
        None
    };

    // Untracked files git itself does not ignore, then our policy on top — and the repo's
    // own answer to which directories hold content, because the policy's list of build-output
    // names is a guess about somebody else's repository and has to lose to what that
    // repository tracks.
    let listed = git(
        Some(path),
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .await?;
    let candidates: Vec<(String, u64)> = listed
        .split('\0')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let size = std::fs::metadata(path.join(p))
                .map(|m| m.len())
                .unwrap_or(0);
            (p.to_string(), size)
        })
        .collect();
    let untracked = select(&candidates, policy, &tracked_dirs(path).await?);

    // Intent-to-add makes the selected untracked files visible to `git diff` as new files,
    // without staging their contents. Reverted below.
    if !untracked.included.is_empty() {
        let mut args = vec!["add", "-N", "--"];
        args.extend(untracked.included.iter().map(String::as_str));
        git(Some(path), &args).await?;
    }

    // `--binary` so a new PNG or fixture survives; `HEAD` so staged and unstaged both come.
    let patch_bytes = git_bytes(Some(path), &["diff", "--binary", "HEAD"]).await;

    // Revert the intent-to-add whatever happened to the diff, so a failed capture does not
    // leave the agent's index altered.
    if !untracked.included.is_empty() {
        let mut args = vec!["reset", "--quiet", "--"];
        args.extend(untracked.included.iter().map(String::as_str));
        let _ = git(Some(path), &args).await;
    }

    let patch = patch_bytes?;
    let patch = if patch.is_empty() { None } else { Some(patch) };

    Ok(Capture {
        bundle,
        patch,
        untracked,
        base_commit: workspace.base_commit.clone(),
        branch: workspace.branch.clone(),
    })
}

/// Every directory this repository keeps tracked files in, at any depth.
///
/// The set, not the file list: it is deduplicated by directory, so a repository with fifty
/// thousand tracked files still yields a few thousand short strings. `-z` for the same reason
/// as everywhere else here — a path we fail to parse is a file we fail to migrate.
async fn tracked_dirs(worktree: &Path) -> Result<BTreeSet<String>, WorkspaceError> {
    let listed = git(Some(worktree), &["ls-files", "-z"]).await?;
    let mut dirs = BTreeSet::new();
    for path in listed.split('\0').filter(|p| !p.is_empty()) {
        let mut prefix = String::new();
        let mut components: Vec<&str> = path.split('/').collect();
        components.pop(); // the file itself
        for component in components {
            if component.is_empty() {
                continue;
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            dirs.insert(prefix.clone());
        }
    }
    Ok(dirs)
}

/// What a restore did with the bundle it was given, and why.
///
/// A `bool` would not do. The three answers are *the agent committed nothing*, *this checkout
/// did not have the work and now does*, and *what is here already contains it* — and the
/// third is the one the caller has to be able to tell from the second, because it is the only
/// one that means a checkout was left alone on purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleOutcome {
    /// The checkpoint carried no bundle: the agent had committed nothing.
    Absent,
    /// Applied. `left_behind` is set when this branch was **not** an ancestor of the bundle's
    /// tip — commits made on a leg the fleet has since moved past. `None` is the ordinary case,
    /// where the branch was simply older.
    Applied { left_behind: Option<LeftBehind> },
    /// Not applied, because this checkout already contains every commit the bundle carries.
    /// A branch that is *ahead* of the checkpoint is the case this protects: resetting onto
    /// the bundle would move the run backwards to where the capture found it.
    AlreadyHere,
}

/// Commits a restore moved past: made on a leg the fleet has since abandoned.
///
/// `reset --hard` does not delete them, and for one version of this that was the whole answer —
/// they are in the reflog, and the hash went into the run's log. Both halves of that are weaker
/// than they sound. A reflog entry for an unreachable commit expires (30 days by default) and an
/// automatic `git gc` is entitled to take it; a hash in a log line requires somebody to know to
/// look, and to still have the log. So the commits are given a **name** as well — the same answer
/// `supersede` gives for a checkout it moves aside rather than deletes, for the same reason:
/// uncommitted or unmerged work is the valuable part, and a thing nobody was told about is a
/// thing nobody finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftBehind {
    /// The commit this branch was on.
    pub commit: String,
    /// The ref keeping it reachable, when one could be written. `None` means the reflog really is
    /// all there is — worth saying rather than implying, because it is the case this exists to
    /// stop relying on.
    pub kept_as: Option<String>,
}

/// Rebuild a captured worktree in `workspace`.
///
/// The workspace must already exist at the capture's base commit — `WorkspaceManager`
/// creates it, and on another node that means cloning the repo first. This applies what
/// the agent did on top.
///
/// **Whether the bundle is applied is decided here, by asking git, and not by the caller.**
/// It used to be decided upstream by whether a branch of that name existed in the mirror,
/// which answers a different question: a branch is *there* whenever this node has ever run a
/// leg of this run, and a branch that is merely older than the checkpoint then silently
/// suppressed the bundle. Measured on two daemons — a run that ran on alpha, migrated, and
/// came back had alpha's six commits and the returning checkout sharing **only the base
/// commit**. The question that was meant is "does what is here already contain the
/// checkpoint's work", and only git can answer it.
pub async fn restore(
    workspace: &Workspace,
    bundle: Option<&[u8]>,
    patch: Option<&[u8]>,
) -> Result<BundleOutcome, WorkspaceError> {
    let path = &workspace.path;
    let mut outcome = BundleOutcome::Absent;

    if let Some(bundle) = bundle {
        // Bundles are fetched from a file, so it has to land on disk first. Inside the
        // worktree's own `.git` area so a crash cannot leave litter in the user's repo.
        let temp = path.join(".offload-restore.bundle");
        std::fs::write(&temp, bundle).map_err(|e| WorkspaceError::Io {
            path: temp.clone(),
            reason: e.to_string(),
        })?;

        let bundle_path = temp.display().to_string();
        // Fetch into a scratch ref, then move the branch onto it. Fetching directly into
        // the checked-out branch is refused by git, and rightly so.
        let result = git(
            Some(path),
            &[
                "fetch",
                "--quiet",
                &bundle_path,
                "+HEAD:refs/offload/restored",
            ],
        )
        .await;
        let _ = std::fs::remove_file(&temp);
        result?;

        // **Ask before resetting.** `reset --hard` is not a merge: onto a branch that is
        // already ahead of the capture it moves the run *backwards*, losing turns nobody
        // asked to lose. `--is-ancestor` is the exact question — is the bundle's tip already
        // in this branch's history — and it is one subprocess.
        let already = git(
            Some(path),
            &[
                "merge-base",
                "--is-ancestor",
                "refs/offload/restored",
                "HEAD",
            ],
        )
        .await
        .is_ok();

        if already {
            outcome = BundleOutcome::AlreadyHere;
        } else {
            // The other direction of the same question, asked only when we are about to move
            // the branch: if HEAD is *not* in the bundle's history either, the two have
            // diverged and this reset leaves commits behind. They stay in the reflog, and a
            // hash somebody was not told is a hash nobody finds.
            let diverged = git(
                Some(path),
                &[
                    "merge-base",
                    "--is-ancestor",
                    "HEAD",
                    "refs/offload/restored",
                ],
            )
            .await
            .is_err();
            let left_behind = if diverged {
                keep_reachable(path, workspace.run).await
            } else {
                None
            };
            git(
                Some(path),
                &["reset", "--hard", "--quiet", "refs/offload/restored"],
            )
            .await?;
            outcome = BundleOutcome::Applied { left_behind };
        }
        let _ = git(Some(path), &["update-ref", "-d", "refs/offload/restored"]).await;
    }

    if let Some(patch) = patch {
        let temp = path.join(".offload-restore.patch");
        std::fs::write(&temp, patch).map_err(|e| WorkspaceError::Io {
            path: temp.clone(),
            reason: e.to_string(),
        })?;

        let patch_path = temp.display().to_string();
        let result = apply_patch(path, &patch_path).await;
        let _ = std::fs::remove_file(&temp);
        result?;
    }

    Ok(outcome)
}

/// Give this branch's current tip a name before something else moves the branch off it.
///
/// Written into `refs/offload/`, which is this project's own namespace in the mirror and is safe
/// from the one thing that deletes refs here: the repo cache is deliberately **not** a
/// `--clone --mirror`, so `fetch --prune` only prunes `refs/remotes/origin/*` (see
/// `ensure_mirror`). The scratch ref a restore already uses lives in the same place.
///
/// Best effort on purpose. Failing to write the ref must not fail the restore — the run continuing
/// matters more than the bookkeeping, and the `None` says which happened.
async fn keep_reachable(path: &Path, run: offload_core::RunId) -> Option<LeftBehind> {
    let head = git(Some(path), &["rev-parse", "HEAD"]).await.ok()?;
    let head = head.trim().to_string();
    let short = head.get(..8).unwrap_or(&head).to_string();
    // The run and the commit both, so a repository holding several of these says which run and
    // which leg without anybody having to resolve a hash.
    let name = format!("refs/offload/left-behind/{run}/{short}");
    let kept_as = git(Some(path), &["update-ref", &name, &head])
        .await
        .map(|_| name)
        .map_err(|e| {
            tracing::warn!(error = %e, commit = %short, "could not name the commits this restore moves past");
        })
        .ok();
    Some(LeftBehind {
        commit: short,
        kept_as,
    })
}

/// Apply a patch, preferring the plain application and falling back to a three-way merge.
///
/// The fallback matters on a receiving node whose repo is at a slightly different state:
/// `git apply` is strict about context, and `--3way` uses the blob hashes in the patch to
/// reconstruct instead of giving up. Failing here means the agent's uncommitted work does
/// not arrive, so it is worth the second attempt.
async fn apply_patch(worktree: &Path, patch_path: &str) -> Result<(), WorkspaceError> {
    match git(
        Some(worktree),
        &["apply", "--whitespace=nowarn", patch_path],
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(first) => {
            tracing::warn!(error = %first, "patch did not apply cleanly; retrying with --3way");
            git(
                Some(worktree),
                &["apply", "--3way", "--whitespace=nowarn", patch_path],
            )
            .await
            .map(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {

    /// The one sentence an operator is ever told about what a capture *carried*.
    ///
    /// It had no test at all, in either direction — no assertion anywhere in the workspace
    /// mentioned `byte bundle`. Found by asking of every checkpoint fixture "what is set to
    /// `None` here, and what would `Some` do": the answer for this function was that its two
    /// interesting branches had never run. The one beside it, `Selection::summary`, has already
    /// cost a pitfall entry for naming the wrong reason, and this is the line that reports it.
    #[test]
    fn a_capture_says_what_it_carried_in_each_of_its_three_shapes() {
        let capture = |bundle: Option<&[u8]>, patch: Option<&[u8]>| Capture {
            bundle: bundle.map(<[u8]>::to_vec),
            patch: patch.map(<[u8]>::to_vec),
            untracked: Selection::default(),
            base_commit: "0f0f0f0f".into(),
            branch: "offload/run-1".into(),
        };

        assert_eq!(
            capture(Some(&[0; 1442]), Some(&[0; 146])).summary(),
            "1442 byte bundle + 146 byte patch (0 untracked file(s))",
            "the migration case: commits and uncommitted work both travel"
        );
        assert_eq!(
            capture(Some(&[0; 1442]), None).summary(),
            "1442 byte bundle (0 untracked file(s))",
            "an agent that committed everything it did"
        );
        assert_eq!(
            capture(None, Some(&[0; 146])).summary(),
            "146 byte patch (0 untracked file(s))",
            "and one that committed nothing"
        );
        assert_eq!(
            capture(None, None).summary(),
            "nothing uncommitted (0 untracked file(s))",
            "a boundary with no work at all is not silent about it"
        );

        // `has_work` is the same question asked for a decision rather than for a sentence, and
        // the two must not drift: a capture the summary calls empty is one nothing should move.
        assert!(!capture(None, None).has_work());
        assert!(capture(Some(&[0; 1]), None).has_work());
        assert!(capture(None, Some(&[0; 1])).has_work());
    }
    use super::*;
    use crate::{RepoSource, WorkspaceManager};
    use offload_core::RunId;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "offload-cp-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("create scratch");
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    async fn source_repo(at: &Path) -> RepoSource {
        std::fs::create_dir_all(at).expect("mkdir");
        git(Some(at), &["init", "--quiet", "--initial-branch=main"])
            .await
            .expect("init");
        git(Some(at), &["config", "user.email", "t@offload.local"])
            .await
            .expect("email");
        git(Some(at), &["config", "user.name", "Offload Test"])
            .await
            .expect("name");
        std::fs::write(at.join("README.md"), "hello\n").expect("write");
        git(Some(at), &["add", "."]).await.expect("add");
        git(Some(at), &["commit", "--quiet", "-m", "initial"])
            .await
            .expect("commit");
        RepoSource::Local(at.to_path_buf())
    }

    async fn identity(workspace: &Workspace) {
        git(
            Some(&workspace.path),
            &["config", "user.email", "t@offload.local"],
        )
        .await
        .expect("email");
        git(
            Some(&workspace.path),
            &["config", "user.name", "Offload Test"],
        )
        .await
        .expect("name");
    }

    #[tokio::test]
    async fn a_clean_worktree_captures_nothing() {
        let scratch = Scratch::new("clean");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let ws = mgr
            .hold(RunId::from_bytes([1; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        let capture = capture(&ws, &UntrackedPolicy::default())
            .await
            .expect("capture");
        assert!(!capture.has_work());
        assert!(capture.bundle.is_none());
        assert!(capture.patch.is_none());
    }

    #[tokio::test]
    async fn uncommitted_edits_and_new_files_both_travel() {
        // The heart of ADR-0003: a modified file and a brand-new one, neither committed.
        // `git diff` alone would miss the new file entirely.
        let scratch = Scratch::new("dirty");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let origin = mgr
            .hold(RunId::from_bytes([2; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(origin.path.join("README.md"), "edited by the agent\n").expect("edit");
        std::fs::write(origin.path.join("new_file.rs"), "fn added() {}\n").expect("create");

        let capture = capture(&origin, &UntrackedPolicy::default())
            .await
            .expect("capture");
        assert!(capture.patch.is_some(), "there is uncommitted work");
        assert_eq!(capture.untracked.included, vec!["new_file.rs"]);

        // Rebuild somewhere else, as a receiving node would.
        let target = mgr
            .hold(RunId::from_bytes([3; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare target");
        restore(&target, None, capture.patch.as_deref())
            .await
            .expect("restore");

        assert_eq!(
            std::fs::read_to_string(target.path.join("README.md")).expect("read"),
            "edited by the agent\n",
            "the edit arrived"
        );
        assert_eq!(
            std::fs::read_to_string(target.path.join("new_file.rs")).expect("read"),
            "fn added() {}\n",
            "and so did the file git did not know about"
        );
    }

    #[tokio::test]
    async fn capture_leaves_the_index_as_it_found_it() {
        // Intent-to-add is only safe because it is reverted. If it were not, the agent's
        // next `git status` would show files it never staged.
        let scratch = Scratch::new("index");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let ws = mgr
            .hold(RunId::from_bytes([4; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(ws.path.join("scratch.rs"), "fn x() {}\n").expect("create");

        let before = git(Some(&ws.path), &["status", "--porcelain"])
            .await
            .expect("status");
        capture(&ws, &UntrackedPolicy::default())
            .await
            .expect("capture");
        let after = git(Some(&ws.path), &["status", "--porcelain"])
            .await
            .expect("status");

        assert_eq!(before, after, "index and worktree unchanged by capture");
    }

    #[tokio::test]
    async fn commits_a_restore_moves_past_are_given_a_name_that_outlives_the_reflog() {
        // **ADR-0053's remaining residual.** When this checkout has commits the bundle does not,
        // the reset moves the branch off them. They survive in the reflog, which expires (30 days
        // for an unreachable commit, and an automatic `gc` may take it sooner) — and the hash went
        // only into a log line, which needs somebody to know to look and to still have the log.
        // So they get a ref, the same answer `supersede` gives a checkout it moves aside.
        let scratch = Scratch::new("diverged");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        // One leg commits and is captured.
        let origin = mgr
            .hold(RunId::from_bytes([8; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        identity(&origin).await;
        std::fs::write(origin.path.join("theirs.rs"), "fn theirs() {}\n").expect("write");
        git(Some(&origin.path), &["add", "."]).await.expect("add");
        git(Some(&origin.path), &["commit", "--quiet", "-m", "theirs"])
            .await
            .expect("commit");
        let capture = capture(&origin, &UntrackedPolicy::default())
            .await
            .expect("capture");
        assert!(capture.bundle.is_some(), "the staging: they committed");

        // Another leg commits something *else* on the same base — the two have diverged, which
        // is what makes this reset destructive rather than a fast-forward.
        let run = RunId::from_bytes([9; 16]);
        let target = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare target");
        identity(&target).await;
        std::fs::write(target.path.join("ours.rs"), "fn ours() {}\n").expect("write");
        git(Some(&target.path), &["add", "."]).await.expect("add");
        git(Some(&target.path), &["commit", "--quiet", "-m", "ours"])
            .await
            .expect("commit");
        let ours = git(Some(&target.path), &["rev-parse", "HEAD"])
            .await
            .expect("rev-parse");
        let ours = ours.trim().to_string();

        let outcome = restore(&target, capture.bundle.as_deref(), None)
            .await
            .expect("restore");

        let BundleOutcome::Applied {
            left_behind: Some(left),
        } = outcome
        else {
            panic!("a diverged branch is exactly the case that leaves commits behind: {outcome:?}");
        };
        assert!(
            ours.starts_with(&left.commit),
            "it names the tip it moved off"
        );
        let kept = left.kept_as.expect("and gives it a name");
        assert!(
            kept.starts_with("refs/offload/"),
            "in this project's own namespace, which `fetch --prune` does not touch: {kept}"
        );

        // The point of all of it: the abandoned work is reachable by name, not by archaeology.
        let resolved = git(Some(&target.path), &["rev-parse", &kept])
            .await
            .expect("the ref resolves");
        assert_eq!(resolved.trim(), ours, "and it points at what we moved off");
        let files = git(
            Some(&target.path),
            &["show", "--name-only", "--format=", &kept],
        )
        .await
        .expect("show");
        assert!(files.contains("ours.rs"), "with the work in it: {files}");

        // …while the checkout itself really did take the other leg's commit.
        assert!(target.path.join("theirs.rs").is_file());
        assert!(
            !target.path.join("ours.rs").is_file(),
            "the reset happened; that is why the ref matters"
        );
    }

    #[tokio::test]
    async fn commits_travel_in_the_bundle() {
        let scratch = Scratch::new("bundle");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let origin = mgr
            .hold(RunId::from_bytes([5; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        identity(&origin).await;
        std::fs::write(origin.path.join("feature.rs"), "fn feature() {}\n").expect("write");
        git(Some(&origin.path), &["add", "."]).await.expect("add");
        git(
            Some(&origin.path),
            &["commit", "--quiet", "-m", "agent's commit"],
        )
        .await
        .expect("commit");

        let capture = capture(&origin, &UntrackedPolicy::default())
            .await
            .expect("capture");
        assert!(capture.bundle.is_some(), "a commit means a bundle");
        assert!(capture.patch.is_none(), "and nothing left uncommitted");

        let target = mgr
            .hold(RunId::from_bytes([6; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare target");
        restore(&target, capture.bundle.as_deref(), None)
            .await
            .expect("restore");

        assert!(target.path.join("feature.rs").is_file(), "commit arrived");
        let log = git(Some(&target.path), &["log", "--oneline", "-1"])
            .await
            .expect("log");
        assert!(log.contains("agent's commit"), "with its message: {log}");
    }

    #[tokio::test]
    async fn commits_and_uncommitted_work_survive_together() {
        // The realistic mid-run state: the agent committed some progress and is partway
        // through the next change when the laptop closes.
        let scratch = Scratch::new("both");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let origin = mgr
            .hold(RunId::from_bytes([7; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        identity(&origin).await;
        std::fs::write(origin.path.join("done.rs"), "fn done() {}\n").expect("write");
        git(Some(&origin.path), &["add", "."]).await.expect("add");
        git(Some(&origin.path), &["commit", "--quiet", "-m", "progress"])
            .await
            .expect("commit");
        std::fs::write(origin.path.join("wip.rs"), "fn half_written(\n").expect("write");

        let capture = capture(&origin, &UntrackedPolicy::default())
            .await
            .expect("capture");
        assert!(capture.bundle.is_some() && capture.patch.is_some());

        let target = mgr
            .hold(RunId::from_bytes([8; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare target");
        restore(&target, capture.bundle.as_deref(), capture.patch.as_deref())
            .await
            .expect("restore");

        assert!(target.path.join("done.rs").is_file(), "committed work");
        assert_eq!(
            std::fs::read_to_string(target.path.join("wip.rs")).expect("read"),
            "fn half_written(\n",
            "and the half-written file, exactly as it was"
        );
    }

    #[tokio::test]
    async fn build_output_is_left_behind_and_said_so() {
        // Phase 1's demo produced 50 untracked files under target/. Carrying those would
        // dwarf the work they surround.
        let scratch = Scratch::new("noise");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let ws = mgr
            .hold(RunId::from_bytes([9; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        std::fs::write(ws.path.join("real_work.rs"), "fn real() {}\n").expect("write");
        std::fs::create_dir_all(ws.path.join("target/debug")).expect("mkdir");
        for i in 0..20 {
            std::fs::write(
                ws.path.join(format!("target/debug/a{i}.o")),
                vec![0u8; 4096],
            )
            .expect("write artefact");
        }

        let capture = capture(&ws, &UntrackedPolicy::default())
            .await
            .expect("capture");

        assert_eq!(capture.untracked.included, vec!["real_work.rs"]);
        assert_eq!(capture.untracked.excluded.len(), 20);
        assert!(
            capture.summary().contains("build output"),
            "the drop has to be visible: {}",
            capture.summary()
        );
    }

    #[tokio::test]
    async fn a_new_file_in_a_directory_the_repo_tracks_travels() {
        // The other half of the case above, through real git rather than a set of strings.
        // This repo keeps hand-written files in `build/` — measured on an ingress chart, and
        // on WordPress's `wp-includes/js/dist/` — so the agent's new file there is work, and
        // the artefacts under `target/` in the same worktree are still not.
        let scratch = Scratch::new("tracked");
        let at = scratch.0.join("src");
        std::fs::create_dir_all(at.join("build")).expect("mkdir");
        std::fs::write(at.join("build/Dockerfile"), "FROM scratch\n").expect("write");
        // `source_repo` commits whatever is there, so `build/Dockerfile` is tracked from the
        // first commit — which is the situation being described.
        let source = source_repo(&at).await;

        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let origin = mgr
            .hold(RunId::from_bytes([13; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(
            origin.path.join("build/Dockerfile.debug"),
            "FROM scratch\nRUN true\n",
        )
        .expect("write");
        std::fs::create_dir_all(origin.path.join("target/debug")).expect("mkdir");
        std::fs::write(origin.path.join("target/debug/x.o"), vec![0u8; 2048]).expect("write");

        let capture = capture(&origin, &UntrackedPolicy::default())
            .await
            .expect("capture");
        assert_eq!(
            capture.untracked.included,
            vec!["build/Dockerfile.debug"],
            "the repo says `build` is content and nothing says that about `target`"
        );

        let target = mgr
            .hold(RunId::from_bytes([14; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare target");
        restore(&target, None, capture.patch.as_deref())
            .await
            .expect("restore");
        assert_eq!(
            std::fs::read_to_string(target.path.join("build/Dockerfile.debug")).expect("read"),
            "FROM scratch\nRUN true\n"
        );
        assert!(!target.path.join("target/debug/x.o").exists());
    }

    #[tokio::test]
    async fn a_binary_file_survives_the_round_trip() {
        // `--binary` on the diff; without it git emits a placeholder and the file arrives
        // empty, which is a silent corruption rather than a failure.
        let scratch = Scratch::new("binary");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let origin = mgr
            .hold(RunId::from_bytes([10; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let bytes: Vec<u8> = (0..=255u8).cycle().take(2048).collect();
        std::fs::write(origin.path.join("fixture.bin"), &bytes).expect("write");

        let capture = capture(&origin, &UntrackedPolicy::default())
            .await
            .expect("capture");

        let target = mgr
            .hold(RunId::from_bytes([11; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare target");
        restore(&target, None, capture.patch.as_deref())
            .await
            .expect("restore");

        assert_eq!(
            std::fs::read(target.path.join("fixture.bin")).expect("read"),
            bytes,
            "binary content is byte-identical"
        );
    }

    #[tokio::test]
    async fn restoring_nothing_is_not_an_error() {
        // A checkpoint of a clean worktree is legitimate: the conversation is the state.
        let scratch = Scratch::new("empty");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let ws = mgr
            .hold(RunId::from_bytes([12; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        restore(&ws, None, None).await.expect("restore nothing");
    }
}
