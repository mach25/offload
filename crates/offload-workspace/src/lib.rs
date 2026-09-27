//! One isolated, disposable git worktree per agent run.
//!
//! Runs get a dedicated branch and directory so two agents on the same repo cannot tread
//! on each other, and so a run can be thrown away without touching anything the user
//! cares about.
//!
//! The layout is built around a **per-repo bare mirror**:
//!
//! ```text
//! <state>/repos/<repo>-<hash>.git/     one bare clone per repo, shared by every run
//! <state>/worktrees/<run>/             one worktree per run, cheap to create and delete
//! ```
//!
//! That structure is why `LocalFacts::workspace_warm` is a real signal rather than a
//! guess: a node that already holds the mirror starts a run in about a second, and only
//! that node knows it. It is exactly the kind of fact gossip carries badly and the node
//! itself knows for free — the argument for bidding in ADR-0006.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod checkpoint;
pub mod peek;
pub mod source;
pub mod status;
pub mod untracked;

pub use checkpoint::Capture;
pub use source::{run_branch, Portability, RepoReach, RepoSource};
pub use status::WorktreeStatus;
pub use untracked::{Selection, UntrackedPolicy};

use offload_core::RunId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("git {args} failed ({code}): {stderr}")]
    Git {
        args: String,
        code: String,
        stderr: String,
    },
    #[error("git is not installed or not on PATH")]
    GitMissing,
    #[error("{0} is not a git repository")]
    NotARepo(PathBuf),
    #[error("io error on {path}: {reason}")]
    Io { path: PathBuf, reason: String },
    /// The workspace is an archive (ADR-0061 §1) whose bytes are not on this node yet.
    ///
    /// Not a failure of this crate — a caller ordering mistake, and it is named rather than
    /// papered over for the reason `Restorable` exists on the resume path: a node that quietly
    /// carried on would build a workspace from nothing and fail somewhere less obvious.
    /// `ensure_archive_mirror` is what acquires it, and the supervisor calls that first.
    #[error(
        "the archive {hash} has not been acquired on this node — its bytes are fetched before \
         the workspace is built"
    )]
    ArchiveNotAcquired { hash: String },
    /// The archive unpacked, and what came out could not be made into a repository.
    #[error("the archive {hash} could not be unpacked into a workspace: {reason}")]
    ArchiveUnusable { hash: String, reason: String },
}

/// The one directory an unpacked archive left behind, if that is all it left.
///
/// `tar czf x.tar dir` archives the directory, not its contents, so unpacking leaves exactly one
/// child holding everything — and both spellings are things an agent will produce. Without this,
/// the second becomes a repository whose only tracked entry is a folder: it works, every
/// mechanism downstream is happy, and the agent finds its files one level deeper than it left
/// them. A failure nobody would go looking for, so it is worth the six lines.
///
/// Deliberately conservative: exactly one entry, and it is a directory. Anything else — files
/// beside it, several directories, an empty archive — is used as it came out.
fn single_child_dir(dir: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(dir).ok()?;
    let first = entries.next()?.ok()?;
    if entries.next().is_some() || !first.path().is_dir() {
        return None;
    }
    Some(first.path())
}

/// A prepared workspace: where the agent runs, and what it started from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub run: RunId,
    /// The agent's working directory. Also determines where its transcript is written —
    /// see `offload_agent::transcript`.
    pub path: PathBuf,
    pub branch: String,
    /// The commit the run branched from. Everything after this is the agent's work, which
    /// is what a checkpoint's git bundle has to carry.
    pub base_commit: String,
}

/// What this node's own copy of a run's worktree is worth, next to a checkpoint.
///
/// Three answers rather than an `Option`, because "there is a checkout here" and "the
/// checkout here is the current one" are different questions and the second one has only
/// been asked since a run could come back to a node it had already left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adoption {
    /// Use it: it holds everything the checkpoint does, and possibly more.
    Current(Workspace),
    /// There is one, and it predates the checkpoint. `at_turn` is where it was left, or
    /// `None` when it never said — which is treated the same way and reported as such.
    Superseded { at_turn: Option<u32> },
    /// Nothing here to adopt.
    Absent,
}

/// Whether tearing a run's worktree down actually tore anything down.
///
/// Two answers rather than `()`, for the same reason [`Adoption`] has three: the caller has
/// something to say afterwards, and it may only say it about a disk it changed. A run's
/// checkout is on exactly one machine and a *finished* run names none — the lease goes with the
/// terminal transition — so "is it here" is a question only this node's own filesystem can
/// answer, and one it has to be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// There was a checkout here, and there is not any more.
    ///
    /// `discarded` is what that checkout held that nothing else has a copy of — the summary
    /// [`crate::status::WorktreeStatus::summary`] gives, with no `commits_ahead` in it because
    /// committed work is on the run branch and survives. `None` is a checkout that held nothing
    /// uncommitted, which is the ordinary case and the one with nothing to say.
    ///
    /// It is measured **before** the removal, for [`CheckoutGuard::supersede`]'s reason: the
    /// teardown is what makes the question unanswerable, so afterwards there is no way to find
    /// out what went. A run that failed mid-turn is exactly the run whose worktree holds edits no
    /// checkpoint has, and `offload rm` is one keystroke from `offload resume`.
    Removed { discarded: Option<String> },
    /// Nothing here to remove: torn down already, or never on this machine at all. The two are
    /// indistinguishable from the record and both are honest as one answer, because what the
    /// caller must not do — claim a removal — is the same in either case.
    NothingHere,
}

/// What became of a checkout a checkpoint from a later leg has moved past.
///
/// Two answers, because "moved aside" is a promise to somebody. The rescue exists for
/// uncommitted work — the part nothing else has a copy of (ADR-0003) — and a checkout that has
/// none is redundant by *measurement*: every file in it is on the run branch in the mirror or
/// arrives with the checkpoint's bundle. Keeping one anyway is not caution, it is a full copy of
/// a tree that nobody will ever be able to tell apart from the copy that mattered.
///
/// **And the question is only askable before the move.** [`CheckoutGuard::supersede`] deletes the
/// rescued copy's `.git` file, because it would point at bookkeeping `worktree prune` is about to
/// remove — so a rescued directory is a plain directory, and `git status` inside one answers
/// `fatal: not a git repository`. Measured. Whatever is not decided here can never be decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rescued {
    /// Moved aside rather than deleted, and where it went. It held work that is nowhere else.
    MovedAside(PathBuf),
    /// Removed. Nothing in it was uncommitted, so the mirror already held every byte of it.
    Redundant,
}

/// Owns this node's repo mirrors and run worktrees.
///
/// Every path that *changes* a run's checkout goes through [`CheckoutGuard`], which is the
/// only way to reach `prepare`, `adopt`, `supersede` and `remove` — see [`Self::hold`] for
/// why that is a type rather than a rule in a comment.
#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
    /// Where new checkouts go (ADR-0074): `~/offload` on a desktop by default, somewhere a person
    /// reads, and `<root>/worktrees` when nothing says otherwise. Mirrors and the per-run turn
    /// markers stay under `root`.
    checkouts: PathBuf,
    /// One mutex per run whose checkout somebody is changing, shared by every clone of this
    /// manager: a lock that a clone does not share is not a lock, and this type is `Clone`.
    ///
    /// Entries are never removed. A `RunId` is 16 bytes and an unlocked `Mutex<()>` is one
    /// word, so the map costs a few dozen bytes per run this daemon has ever built a
    /// checkout for — and reaping an entry means proving nobody is about to take it, which
    /// is the very race this exists to close.
    held: Arc<Mutex<HashMap<RunId, Arc<tokio::sync::Mutex<()>>>>>,
}

impl WorkspaceManager {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let checkouts = root.join("worktrees");
        Self::with_checkouts(root, checkouts)
    }

    /// Mirrors under `root`, and run checkouts in `checkouts` (ADR-0074).
    #[must_use]
    pub fn with_checkouts(root: PathBuf, checkouts: PathBuf) -> Self {
        WorkspaceManager {
            root,
            checkouts,
            held: Arc::default(),
        }
    }

    /// Take exclusive hold of one run's checkout, waiting for whoever has it.
    ///
    /// **The four operations that change a checkout are on the guard, not on this type**, so
    /// the compiler asks for the lock rather than a comment asking the next session to
    /// remember it. That is deliberate: the guard a caller needs spans *several* calls —
    /// `restore_workspace` asks `adopt`, then `supersede`, then `prepare` — so an internal
    /// lock taken per method would serialise each one and protect none of the sequence.
    ///
    /// It is a lock rather than a check because the thing being guarded is a **subprocess**.
    /// `remove` is `git worktree remove --force`, ~2ms of somebody else's program, and a
    /// guard read before it is on the wrong side of it: measured on a forced pair, every
    /// rebuild starting 0–1ms into a removal adopted a checkout that was gone by the time it
    /// returned, and every one starting 2ms or later was fine. A window it loses every time,
    /// which is what a fence cannot help with.
    ///
    /// **What it does not cover, on purpose**: the agent's own lifetime. Holding this for the
    /// hours a run lasts would stop the sweep dead, and it is not needed — a rebuild
    /// *registers* the run before it touches the disk (`Supervisor::register`, ADR-0051), so
    /// every question the sweep asks about a live run already answers correctly. This closes
    /// the gap between "the checkout exists" and "the run is claimed", and nothing wider.
    ///
    /// In-process only. Two daemons over one state directory would need a file lock, and
    /// that is not this: a node owns its state directory.
    pub async fn hold(&self, run: RunId) -> CheckoutGuard<'_> {
        let mutex = self.mutex_for(run);
        let held = mutex.lock_owned().await;
        CheckoutGuard {
            mgr: self,
            run,
            _held: held,
        }
    }

    /// Take hold of a checkout only if it is free, for a caller with nothing to wait for.
    ///
    /// The checkout sweep's door. Waiting would be wrong there twice over: the tick would
    /// block for as long as a rebuild takes, and a checkout somebody is *rebuilding* is
    /// precisely one the sweep must leave alone — so "busy" is a complete answer and the next
    /// tick is soon enough. An operator command wanting the same disk should [`Self::hold`]
    /// and wait, because somebody is there to be told what happened.
    pub fn try_hold(&self, run: RunId) -> Option<CheckoutGuard<'_>> {
        let mutex = self.mutex_for(run);
        let held = mutex.try_lock_owned().ok()?;
        Some(CheckoutGuard {
            mgr: self,
            run,
            _held: held,
        })
    }

    fn mutex_for(&self, run: RunId) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = match self.held.lock() {
            Ok(map) => map,
            // A poisoned map is a panic in a *different* run's guard construction, which
            // holds nothing of ours. Refusing every checkout for ever afterwards would turn
            // one bad run into a node that can neither start nor reclaim anything.
            Err(poisoned) => poisoned.into_inner(),
        };
        Arc::clone(map.entry(run).or_default())
    }

    #[must_use]
    pub fn repos_dir(&self) -> PathBuf {
        self.root.join("repos")
    }

    /// Where new checkouts are made.
    #[must_use]
    pub fn worktrees_dir(&self) -> PathBuf {
        self.checkouts.clone()
    }

    /// Where checkouts were made before ADR-0074, and where this node's own bookkeeping beside
    /// them (turn markers) still lives: under the state directory, out of a person's way.
    fn internal_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    /// Every run that has a checkout on this machine, whatever the fleet thinks of it.
    ///
    /// Asked of the disk rather than of the store, because that is the question: a checkout
    /// outlives the record's interest in it, and the ones worth reclaiming are precisely the ones
    /// nothing points at any more. Turn markers sit beside the worktrees rather than inside them
    /// ([`WorkspaceManager::turn_marker`]), so anything that is not a directory named by a run id
    /// is skipped.
    #[must_use]
    pub fn checkouts(&self) -> Vec<RunId> {
        // Both places (ADR-0074): a checkout made before the move stays where it was, and is as
        // much this node's as one made since. Nothing is moved, so nothing can be lost moving it.
        let mut dirs = vec![self.worktrees_dir()];
        if self.internal_dir() != self.worktrees_dir() {
            dirs.push(self.internal_dir());
        }
        let mut found: Vec<RunId> = dirs
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flat_map(|entries| entries.flatten())
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().to_str()?.parse::<RunId>().ok())
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }

    #[must_use]
    pub fn mirror_path(&self, source: &RepoSource) -> PathBuf {
        self.repos_dir().join(format!("{}.git", source.cache_key()))
    }

    /// Where this run's checkout is: where it was made before ADR-0074 if it is still there,
    /// else where new ones go. Asked of the disk, so a run whose checkout predates the move is
    /// adopted, checkpointed and reclaimed where it is, never rebuilt beside itself.
    #[must_use]
    pub fn worktree_path(&self, run: RunId) -> PathBuf {
        let before = self.internal_dir().join(run.to_string());
        if self.internal_dir() != self.worktrees_dir() && before.is_dir() {
            return before;
        }
        self.worktrees_dir().join(run.to_string())
    }

    /// Does this node already hold the repo?
    ///
    /// Feeds `LocalFacts::workspace_warm` at bid time. Cheap on purpose — it is called
    /// while deciding whether to bid, not while running.
    #[must_use]
    pub fn is_warm(&self, source: &RepoSource) -> bool {
        self.mirror_path(source).join("HEAD").is_file()
    }

    /// Can this node get the repo at all, and if not, why not?
    ///
    /// The eligibility half of `is_warm`, and the two are genuinely different questions: warm
    /// is "would this be fast", reachable is "is this possible here". A clone URL is obtainable
    /// everywhere — that is what `Portability::Fleetwide` means. A local path is obtainable
    /// exactly where it exists, or where a mirror of it was already made, and nowhere else.
    ///
    /// Feeds `LocalFacts::repo_reach`, which refuses the bid rather than accepting a run
    /// that would fail when somebody eventually tried to start it.
    ///
    /// It answers *why* because the refusal is read at a keyboard. A directory that is here and
    /// is not a repository used to be refused with the sentence written for a path on another
    /// machine — by the machine it was sitting on — because the message was computed from
    /// `RepoSource::parse`, which cannot see a filesystem. This can, and it is the only thing in
    /// the path that can, so the answer starts here.
    #[must_use]
    pub fn reach(&self, source: &RepoSource) -> RepoReach {
        match source {
            RepoSource::Remote(_) => RepoReach::Obtainable,
            // Obtainable everywhere, for the reason a clone URL is: the bytes are named by a
            // hash any node can ask a peer for (ADR-0061 §5 — the bid carries the digest, the
            // transfer happens on acceptance). Whether *this* node already holds them is
            // `is_warm`'s question, not this one; whether a peer still has them is answered by
            // the fetch, and a fetch that fails fails the run exactly as a failed clone does.
            RepoSource::Archive(_) => RepoReach::Obtainable,
            // Any node can make an empty repository (ADR-0072).
            RepoSource::Scratch => RepoReach::Obtainable,
            RepoSource::Local(path) => {
                if crate::source::is_git_repo(path) || self.is_warm(source) {
                    RepoReach::Obtainable
                } else if path.exists() {
                    RepoReach::NotARepo
                } else {
                    RepoReach::NotHere
                }
            }
        }
    }

    /// Clone the repo if we don't have it, refresh it if we do.
    ///
    /// Deliberately **not** `clone --mirror`. A mirror's refspec is `+refs/*:refs/*`, so
    /// the next `fetch --prune` would delete every `offload/run-*` branch that doesn't
    /// exist upstream — which is all of them. Instead upstream branches are mapped under
    /// `refs/remotes/origin/*`, leaving `refs/heads/*` as ours alone.
    ///
    /// **The mirror appears all at once or not at all.** It is built under a scratch name and
    /// renamed into place, because the alternative was found by submitting three runs for a
    /// cold repo at once: `git clone` into the final path is not atomic, so two of the three
    /// died on `destination path already exists` while the first was still cloning. Worse than
    /// the failure was its shape — a half-written directory is not a mirror and has no `HEAD`,
    /// so *every later run on that repo* would try to clone over it and fail the same way, on
    /// a node that looks perfectly healthy. A rename cannot half-happen, and losing the race
    /// is not an error: whoever got there first built the same thing.
    pub async fn ensure_mirror(&self, source: &RepoSource) -> Result<PathBuf, WorkspaceError> {
        let mirror = self.mirror_path(source);

        if mirror.join("HEAD").is_file() {
            // An archive's mirror is **immutable**, so there is nothing to refresh: the name of
            // the thing is the hash of its bytes, and the origin it was built from was a scratch
            // directory that no longer exists. Fetching would fail every time, survivably and
            // noisily, and the warning below would say a mirror refresh failed about a mirror
            // that is exactly as current as it will ever be.
            // A scratch mirror is the same: one empty commit, the same on every node for ever.
            if source.archive().is_some() || *source == RepoSource::Scratch {
                return Ok(mirror);
            }
            tracing::debug!(mirror = %mirror.display(), "refreshing existing mirror");
            // A fetch failure is survivable: an offline node can still run against the
            // objects it already has. Failing the run here would make a transient network
            // problem look like an unplaceable run.
            if let Err(e) = git(Some(&mirror), &["fetch", "--prune", "--quiet", "origin"]).await {
                tracing::warn!(error = %e, "mirror refresh failed; continuing with cached objects");
            }
            return Ok(mirror);
        }

        if let RepoSource::Local(path) = source {
            if !source::is_git_repo(path) {
                return Err(WorkspaceError::NotARepo(path.clone()));
            }
        }
        // An archive is never *cloned* into existence: there is nothing to clone from, which is
        // the whole reason ADR-0061 exists. Its mirror is built once from bytes this node has
        // already acquired, by `ensure_archive_mirror`, and the branch above returns it on every
        // later call. Reaching here means the bytes are not here yet, and saying so is better
        // than building an empty workspace and failing a turn later.
        if let RepoSource::Archive(hash) = source {
            return Err(WorkspaceError::ArchiveNotAcquired {
                hash: hash.to_string(),
            });
        }

        std::fs::create_dir_all(self.repos_dir()).map_err(|e| WorkspaceError::Io {
            path: self.repos_dir(),
            reason: e.to_string(),
        })?;

        let scratch = self.scratch_mirror_path(source);
        // A scratch directory from a crashed clone is worth nothing and would fail the same
        // way the old code did, so it goes.
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).map_err(|e| WorkspaceError::Io {
                path: scratch.clone(),
                reason: e.to_string(),
            })?;
        }

        let built = if *source == RepoSource::Scratch {
            // Made, not cloned (ADR-0072): there is no origin, and nothing to fetch.
            tracing::info!("making the empty repository scratch runs start from");
            build_scratch_mirror(&scratch).await
        } else {
            tracing::info!(repo = %source.clone_target(), "cloning repo mirror");
            self.build_mirror(source, &scratch).await
        };
        if built.is_err() {
            let _ = std::fs::remove_dir_all(&scratch);
        }
        built?;

        // Losing the race is the ordinary outcome of two runs starting together, and the
        // winner's mirror is as good as ours. Rename first and ask afterwards: checking
        // whether the path exists and then renaming is the same race one level up.
        if let Err(e) = std::fs::rename(&scratch, &mirror) {
            let _ = std::fs::remove_dir_all(&scratch);
            if !mirror.join("HEAD").is_file() {
                return Err(WorkspaceError::Io {
                    path: mirror,
                    reason: e.to_string(),
                });
            }
            tracing::debug!(mirror = %mirror.display(), "another run cloned it first");
        }

        Ok(mirror)
    }

    /// Build this node's mirror of an archive workspace, from bytes it already holds.
    ///
    /// ADR-0061 §2, and the clause the rest of the decision rests on. What arrives is *some
    /// bytes an agent chose*; what this leaves behind is **an ordinary bare mirror**, so every
    /// mechanism downstream applies with no change at all: `prepare`'s worktree and run branch,
    /// the `base..HEAD` bundle, the patch of uncommitted work, `restore`'s `merge-base`
    /// reasoning (ADR-0053), replication, `here only` durability, and migration to a third node.
    /// The agent's judgement is needed here and never again.
    ///
    /// Two shapes go in and one comes out:
    ///
    /// * the archive **contains a `.git`**, so it already is a repository and its history is
    ///   kept — which is the `client-plugins` case, 4.5 MB of history against a 3.6 GB tree;
    /// * it does not, so this node runs `git init` and commits the contents as the base — which
    ///   is the root directory the whole ADR is about.
    ///
    /// **Under the node's state dir, never in the submitter's directory** (§2). Nothing here
    /// writes outside `repos_dir()`.
    ///
    /// Idempotent and safe to race, by the same scratch-then-rename argument `ensure_mirror`
    /// makes at length: two runs submitted from one archive build the same thing, and losing
    /// the race means somebody else built it.
    pub async fn ensure_archive_mirror(
        &self,
        source: &RepoSource,
        archive: &Path,
    ) -> Result<(PathBuf, Option<Acquired>), WorkspaceError> {
        let Some(hash) = source.archive() else {
            // Not reachable from the supervisor, and typed rather than ignored: silently
            // treating a `Local` path as an archive would unpack somebody's repository over
            // itself.
            return Err(WorkspaceError::ArchiveUnusable {
                hash: source.clone_target(),
                reason: "this workspace is not an archive".into(),
            });
        };
        let hash = hash.to_string();
        let mirror = self.mirror_path(source);
        if mirror.join("HEAD").is_file() {
            // Already acquired, so nothing travelled *now* and there is nothing to report. The
            // `None` is the honest answer: a second run on one archive must not log a second
            // acquisition, and §3 is precisely that the bytes move once per node.
            return Ok((mirror, None));
        }

        std::fs::create_dir_all(self.repos_dir()).map_err(|e| WorkspaceError::Io {
            path: self.repos_dir(),
            reason: e.to_string(),
        })?;
        let scratch = self.scratch_mirror_path(source);
        let work = scratch.with_extension("unpacked");
        for dir in [&scratch, &work] {
            if dir.exists() {
                std::fs::remove_dir_all(dir).map_err(|e| WorkspaceError::Io {
                    path: dir.clone(),
                    reason: e.to_string(),
                })?;
            }
        }
        std::fs::create_dir_all(&work).map_err(|e| WorkspaceError::Io {
            path: work.clone(),
            reason: e.to_string(),
        })?;

        tracing::info!(archive = %hash, "unpacking an archive workspace");
        let built = self
            .unpack_into_mirror(&hash, archive, &work, &scratch)
            .await;
        // The unpacked tree is scaffolding: the mirror holds everything that matters, and on a
        // 512 MiB archive this is the difference between one copy on disk and two.
        let _ = std::fs::remove_dir_all(&work);
        if built.is_err() {
            let _ = std::fs::remove_dir_all(&scratch);
        }
        let acquired = built?;

        if let Err(e) = std::fs::rename(&scratch, &mirror) {
            let _ = std::fs::remove_dir_all(&scratch);
            if !mirror.join("HEAD").is_file() {
                return Err(WorkspaceError::Io {
                    path: mirror,
                    reason: e.to_string(),
                });
            }
            tracing::debug!(archive = %hash, "another run unpacked it first");
        }
        Ok((mirror, Some(acquired)))
    }

    /// Unpack, make it a repository if it is not one, and bare-clone it into `at`.
    async fn unpack_into_mirror(
        &self,
        hash: &str,
        archive: &Path,
        work: &Path,
        at: &Path,
    ) -> Result<Acquired, WorkspaceError> {
        let unusable = |reason: String| WorkspaceError::ArchiveUnusable {
            hash: hash.to_string(),
            reason,
        };

        // `tar` rather than a crate, for the reason this whole crate shells out to `git`: it is
        // the tool the agent on the other end used, and reading its exit status is less surface
        // than owning an archive parser. `-f` with a path rather than stdin so the bytes are
        // never held in this process — a 512 MiB archive costs the blob plane its whole size in
        // RAM already (measured, session seventy-eight) and there is no reason to pay it twice.
        let out = tokio::process::Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(work)
            .output()
            .await
            .map_err(|e| unusable(format!("could not run tar: {e}")))?;
        if !out.status.success() {
            return Err(unusable(format!(
                "tar exited {}: {}",
                out.status
                    .code()
                    .map_or_else(|| "on a signal".to_string(), |c| c.to_string()),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }

        // An archive of `dir/` rather than of its contents is the ordinary thing `tar czf x.tar
        // dir` produces, and unpacking it leaves one directory holding everything. Descend, so
        // both spellings work — the alternative is a repository whose only content is a folder,
        // which would "work" and be wrong in a way nobody would look for.
        let root = single_child_dir(work).unwrap_or_else(|| work.to_path_buf());

        mirror_from_tree(&root, at, "the archive this run started from").await?;
        // From the tree rather than from the tar's own listing: what matters to somebody
        // diagnosing a missing file is what is *there*, and the two can differ — a tar may carry
        // entries that do not land, and the descent into a single root above changes what the
        // paths mean.
        Ok(describe(&root))
    }

    /// Clone into `at` and set the refspec that keeps run branches ours.
    async fn build_mirror(&self, source: &RepoSource, at: &Path) -> Result<(), WorkspaceError> {
        git(
            None,
            &[
                "clone",
                "--bare",
                "--quiet",
                &source.clone_target(),
                &at.display().to_string(),
            ],
        )
        .await?;

        git(
            Some(at),
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )
        .await?;
        // Populate refs/remotes/origin/* so start points resolve on the very first run.
        git(Some(at), &["fetch", "--prune", "--quiet", "origin"]).await?;
        Ok(())
    }

    /// Where a mirror is assembled before it is renamed into place.
    ///
    /// Named per process and per call so two runs — or two daemons sharing a state dir by
    /// accident — never build into the same scratch directory.
    fn scratch_mirror_path(&self, source: &RepoSource) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.repos_dir().join(format!(
            ".{}.{}.{n}.partial",
            source.cache_key(),
            std::process::id()
        ))
    }

    /// The repo's default branch, as the remote reports it.
    pub async fn default_branch(&self, source: &RepoSource) -> Result<String, WorkspaceError> {
        let mirror = self.mirror_path(source);
        let head = git(Some(&mirror), &["symbolic-ref", "--short", "HEAD"]).await?;
        Ok(head.trim().to_string())
    }

    /// Where this run's last-known turn count is recorded, beside its worktree.
    ///
    /// Outside the worktree on purpose: a marker file *inside* it would be untracked, so it
    /// would show up in `git status`, in the checkpoint's own untracked selection, and in
    /// every patch this run ever produced.
    fn turn_marker(&self, run: RunId) -> PathBuf {
        self.internal_dir().join(format!("{run}.turn"))
    }

    /// Record that this node's worktree for `run` now holds the work up to `turns`.
    ///
    /// Written whenever the worktree and a checkpoint are made to agree — after a capture,
    /// and after a restore — because both are moments where "what is on this disk" changes
    /// what a later resume may safely adopt. Only ever forward: a restore of an older
    /// checkpoint does not make the checkout older than the turns it already holds.
    pub fn note_turn(&self, run: RunId, turns: u32) {
        if self.worktree_turn(run).is_some_and(|at| at >= turns) {
            return;
        }
        let path = self.turn_marker(run);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, turns.to_string()) {
            // Losing this is not fatal — it reads as "unknown", which declines to adopt —
            // so it is a warning rather than a failed checkpoint.
            tracing::warn!(run_id = %run, error = %e, "could not record the worktree's turn");
        }
    }

    /// A finished run's worktree on this node, as a [`Workspace`] a capture can read — or `None`
    /// when there is no checkout here (ADR-0064 §2).
    ///
    /// `base` may be the whole commit or the prefix a log row kept; it is resolved **in the
    /// worktree** to a full id, and refused if it does not name a commit there, because a capture
    /// against the wrong base bundles the wrong range and nothing downstream could tell.
    pub async fn finished_worktree(
        &self,
        run: RunId,
        base: &str,
    ) -> Result<Option<Workspace>, WorkspaceError> {
        let path = self.worktree_path(run);
        if !path.join(".git").exists() {
            return Ok(None);
        }
        let spec = format!("{base}^{{commit}}");
        let base_commit = git(Some(&path), &["rev-parse", "--verify", "--quiet", &spec]).await?;
        Ok(Some(Workspace {
            run,
            path,
            branch: run_branch(run),
            base_commit: base_commit.trim().to_string(),
        }))
    }

    /// Keep `.offload/` out of git in every worktree of this repository's mirror (ADR-0064 §3).
    ///
    /// Through the mirror's `info/exclude`, which git reads from the common directory and so
    /// applies to every worktree — measured in ADR-0064's amendment: `ls-files --others
    /// --exclude-standard`, `status` and `add -A` all skip it. So a continuation's copy of its
    /// parent's transcript is never committed, never in a patch, and not re-shipped at every turn
    /// boundary. Idempotent: the line is added once.
    pub fn exclude_offload_dir(&self, source: &RepoSource) -> Result<(), WorkspaceError> {
        let info = self.mirror_path(source).join("info");
        let exclude = info.join("exclude");
        let current = std::fs::read_to_string(&exclude).unwrap_or_default();
        if current.lines().any(|line| line.trim() == "/.offload/") {
            return Ok(());
        }
        std::fs::create_dir_all(&info).map_err(|e| WorkspaceError::Io {
            path: info.clone(),
            reason: e.to_string(),
        })?;
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("# Offload's own files in a run's worktree (ADR-0064)\n/.offload/\n");
        std::fs::write(&exclude, text).map_err(|e| WorkspaceError::Io {
            path: exclude,
            reason: e.to_string(),
        })
    }

    /// The turn this node's worktree was last recorded at, if it says.
    #[must_use]
    pub fn worktree_turn(&self, run: RunId) -> Option<u32> {
        std::fs::read_to_string(self.turn_marker(run))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// A name no superseded checkout has taken yet.
    ///
    /// It can happen twice: a run can leave and come back more than once. Renaming onto an
    /// existing directory would fail, and picking the same name twice would destroy the
    /// first rescue, which is the one thing this must not do.
    fn free_superseded_path(&self, run: RunId) -> PathBuf {
        // Beside the checkout it rescues, wherever that is: a person looking for their work
        // finds it next to where it was.
        let dir = self
            .worktree_path(run)
            .parent()
            .map_or_else(|| self.worktrees_dir(), Path::to_path_buf);
        let base = dir.join(format!("{run}.superseded"));
        if !base.exists() {
            return base;
        }
        for n in 2..1_000 {
            let candidate = dir.join(format!("{run}.superseded.{n}"));
            if !candidate.exists() {
                return candidate;
            }
        }
        base
    }

    /// Does the run's branch still exist in the mirror?
    ///
    /// Distinguishes the two rebuild paths: with the branch, the agent's commits are still
    /// here and only uncommitted work needs restoring. Without it — a fresh node, or a
    /// mirror that was cleaned — the checkpoint's bundle is the only copy.
    pub async fn has_branch(&self, source: &RepoSource, branch: &str) -> bool {
        let mirror = self.mirror_path(source);
        git(Some(&mirror), &["rev-parse", "--verify", "--quiet", branch])
            .await
            .is_ok_and(|out| !out.trim().is_empty())
    }

    /// What has the agent changed?
    pub async fn status(&self, ws: &Workspace) -> Result<WorktreeStatus, WorkspaceError> {
        let raw = git(
            Some(&ws.path),
            &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
        )
        .await?;
        let mut status = status::parse_porcelain(&raw);

        let range = format!("{}..HEAD", ws.base_commit);
        if let Ok(count) = git(Some(&ws.path), &["rev-list", "--count", &range]).await {
            status.commits_ahead = count.trim().parse().unwrap_or(0);
        }

        Ok(status)
    }

    /// Whether a run's checkout on **this** node still holds work nothing else has a copy of.
    ///
    /// `None` when the checkout is not here at all, which is a different answer from "no" and has
    /// to stay one: a caller that reads a missing worktree as "nothing to lose" is making a claim
    /// about a machine it cannot see.
    ///
    /// Uncommitted only, deliberately. Committed work is on the run branch in the mirror and
    /// survives teardown — [`WorkspaceManager::remove`] says so — so the fragile part is exactly
    /// the part `git status` reports, which is also the part ADR-0003 calls the valuable one. It
    /// asks git rather than reading the summary beside the run: that string is written at a turn
    /// boundary and is one boundary stale by construction, and this is the question whose wrong
    /// answer deletes somebody's work.
    pub async fn holds_uncommitted(&self, run: RunId) -> Option<bool> {
        let path = self.worktree_path(run);
        if !path.exists() {
            return None;
        }
        let raw = git(
            Some(&path),
            &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
        )
        .await
        // A worktree we cannot read is one we must not decide about. `Some(true)` is the safe
        // direction — it keeps the checkout — and it is the one this returns.
        .ok()?;
        Some(status::parse_porcelain(&raw).is_dirty())
    }
}

/// Exclusive hold on one run's checkout, and the only way to change one.
///
/// Held across a *sequence* rather than a call, which is the whole point: a resume asks
/// `adopt`, may `supersede` what it finds, and then `prepare`s a replacement, and a lock
/// taken inside each of those three would leave the gaps between them open. Released on
/// drop, so the sequence ending — by return, by `?`, or by panic — frees the checkout.
///
/// It does **not** guard the contents of the checkout once it exists: `checkpoint::capture`
/// and `checkpoint::restore` take a [`Workspace`], which only `prepare` and `adopt` hand
/// out, and the agent writing into its own worktree is the run's business. What this
/// serialises is the directory coming into and going out of existence.
#[derive(Debug)]
pub struct CheckoutGuard<'a> {
    mgr: &'a WorkspaceManager,
    run: RunId,
    /// Dropped with the guard, which is what releases the checkout. Never read.
    _held: tokio::sync::OwnedMutexGuard<()>,
}

impl CheckoutGuard<'_> {
    /// The run whose checkout this holds.
    ///
    /// Exists so a caller that took the guard first does not carry the id twice: the four
    /// operations below read it from here, and a guard for one run cannot be used to change
    /// another run's checkout — which is the mismatch a `run: RunId` parameter beside a
    /// `&CheckoutGuard` would have let through.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }

    /// Create the run's worktree and branch.
    ///
    /// `git_ref` is where to start from; `None` means the repo's default branch.
    pub async fn prepare(
        &self,
        source: &RepoSource,
        git_ref: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Workspace, WorkspaceError> {
        let run = self.run;
        let mirror = self.mgr.ensure_mirror(source).await?;
        let path = self.mgr.worktree_path(run);
        let branch = branch.map_or_else(|| run_branch(run), str::to_string);

        let start = match git_ref {
            Some(r) => r.to_string(),
            None => format!(
                "refs/remotes/origin/{}",
                self.mgr.default_branch(source).await?
            ),
        };
        let base_commit = git(Some(&mirror), &["rev-parse", &start]).await?;
        let base_commit = base_commit.trim().to_string();

        std::fs::create_dir_all(self.mgr.worktrees_dir()).map_err(|e| WorkspaceError::Io {
            path: self.mgr.worktrees_dir(),
            reason: e.to_string(),
        })?;

        // `-B` rather than `-b`: after a crash the branch may already exist, and a run
        // that cannot restart because of leftover state from its own previous attempt is
        // a bad failure mode.
        git(
            Some(&mirror),
            &[
                "worktree",
                "add",
                "--quiet",
                "-B",
                &branch,
                &path.display().to_string(),
                &base_commit,
            ],
        )
        .await?;

        tracing::info!(
            run_id = %run,
            path = %path.display(),
            branch = %branch,
            base = %&base_commit[..base_commit.len().min(8)],
            "workspace ready"
        );

        Ok(Workspace {
            run,
            path,
            branch,
            base_commit,
        })
    }

    /// Adopt a worktree this node already has on disk — *if* it is the current one.
    ///
    /// The resume path's first question: is the run's checkout still here? If it is, and the
    /// run never left, it is at least as current as the checkpoint — the agent stopped, the
    /// files did not move — so rebuilding it from blobs would replace live state with a
    /// staler copy.
    ///
    /// **A run that comes back makes that reasoning false**, which is why this takes the
    /// checkpoint's turn count. Nothing removes a worktree at the moment a run leaves this node:
    /// `cleanup` is deliberately manual and only for terminal runs, and the checkout sweep
    /// (`Supervisor::reclaim_departed_checkouts`) keeps anything a person may still read.
    /// So the checkout from an earlier leg sits there indefinitely, and a run that migrated
    /// away, did nineteen more turns elsewhere and came home would be adopted at turn one —
    /// silently, with the log saying "adopted in place". Turn counts continue across legs
    /// (they are the run's, not a process's), so they are the one number that orders two
    /// legs' worth of state without a clock or a message.
    #[must_use]
    pub fn adopt(&self, branch: &str, base_commit: &str, through: u32) -> Adoption {
        let run = self.run;
        let path = self.mgr.worktree_path(run);
        // `.git` in a worktree is a file pointing at the mirror, not a directory.
        if !path.join(".git").exists() {
            return Adoption::Absent;
        }
        // Equal turns is current: it means this worktree produced the checkpoint, or the
        // other node captured the same boundary again without the agent advancing.
        //
        // No marker at all is *not* treated as current, because that is exactly what a
        // worktree written by a build that had no markers looks like — and the unsafe
        // reading is the confident one. What it costs is a rebuild on the first resume of a
        // run that was in flight across the upgrade; what it buys is that adoption never
        // rests on an assumption nothing recorded. Nothing is destroyed either way:
        // `supersede` moves the old checkout aside rather than deleting it.
        let at = self.mgr.worktree_turn(run);
        if at.is_none_or(|at| at < through) {
            return Adoption::Superseded { at_turn: at };
        }
        Adoption::Current(Workspace {
            run,
            path,
            branch: branch.to_string(),
            base_commit: base_commit.to_string(),
        })
    }

    /// Deal with a checkout a later leg's checkpoint has moved past: rescue it, or remove it.
    ///
    /// Not simply `remove`. The reason this path exists at all is that uncommitted work is the
    /// valuable part, and a checkout the fleet has moved past may still hold the only copy of
    /// something — a mid-turn edit that never made it into a checkpoint. So it is renamed rather
    /// than deleted, and the operator is told the path in the run's own log.
    ///
    /// **But it asks first, and it has to ask here.** For two phases this renamed
    /// unconditionally, which sounds like the cautious choice and is not: the rename removes the
    /// rescued copy's `.git` file (below), so from that moment nothing in the product or outside
    /// it can tell whether the directory held anything the mirror did not. Measured — `git status`
    /// in a rescued copy answers `fatal: not a git repository`. So every rescue was treasure for
    /// ever, three legs of one run left three full copies of the tree, and nothing removed any of
    /// them. [`WorkspaceManager::holds_uncommitted`] is the question, asked while this is still a
    /// worktree and answered by git; a checkout holding nothing goes through the ordinary
    /// teardown, since its commits are on the run branch and `remove` says so.
    ///
    /// **Unknown is not nothing.** `holds_uncommitted` answers `None` for a worktree it could not
    /// read, and that keeps the copy — the same direction it takes for its other caller, and the
    /// only safe one at a door whose wrong answer deletes somebody's only copy of a file.
    ///
    /// Git's registration goes with the rename: `prune` drops the entry whose `gitdir` no longer
    /// resolves, which is what frees the run's branch for the rebuilt worktree. The rescued
    /// copy's `.git` file would point at bookkeeping that is gone, so it goes too — what is
    /// left is a plain directory of files, which is what it is for.
    pub async fn supersede(&self, source: &RepoSource) -> Result<Rescued, WorkspaceError> {
        let run = self.run;
        let path = self.mgr.worktree_path(run);

        // Asked before anything moves, because after the rename it is unanswerable. `Some(false)`
        // is the only answer that removes: `None` is a worktree git would not talk about, and
        // deciding a rescue on a question that failed is how the copy that mattered goes.
        if self.mgr.holds_uncommitted(run).await == Some(false) {
            let removal = self.remove(source).await?;
            tracing::info!(
                run_id = %run,
                ?removal,
                "worktree superseded by a checkpoint from a later leg; it held nothing \
                 uncommitted, so the mirror already had all of it"
            );
            return Ok(Rescued::Redundant);
        }

        let moved = self.mgr.free_superseded_path(run);

        std::fs::rename(&path, &moved).map_err(|e| WorkspaceError::Io {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        let _ = std::fs::remove_file(moved.join(".git"));
        let _ = std::fs::remove_file(self.mgr.turn_marker(run));
        let _ = git(Some(&self.mgr.mirror_path(source)), &["worktree", "prune"]).await;

        tracing::warn!(
            run_id = %run,
            path = %moved.display(),
            "worktree superseded by a checkpoint from a later leg; it held uncommitted work, \
             so it was moved aside"
        );
        Ok(Rescued::MovedAside(moved))
    }

    /// Tear down a run's worktree.
    ///
    /// The branch is left behind on purpose — it holds the agent's commits, and deleting
    /// it would discard work that a checkpoint may still need to bundle. Branch cleanup
    /// belongs to whoever decides the run is finished with, not to worktree teardown.
    pub async fn remove(&self, source: &RepoSource) -> Result<Removal, WorkspaceError> {
        let run = self.run;
        let mirror = self.mgr.mirror_path(source);
        let path = self.mgr.worktree_path(run);
        if !path.exists() {
            // Still `Ok` — tearing down twice is not a fault, and neither is tearing down on a
            // machine that never ran this leg. But the two are not "done": the caller has a
            // worktree summary to write, and writing one for a checkout this node does not have
            // is a claim about another machine's disk.
            return Ok(Removal::NothingHere);
        }

        // **Asked before the teardown, because the teardown is what makes it unanswerable.**
        // ADR-0055's rule, met on the one path that had never applied it: the sweep asks
        // `holds_uncommitted` and keeps such a checkout, `supersede` asks and moves it aside, and
        // this — the path an operator types — asked nothing and said "the run's branch and
        // commits are kept", which is the reassuring half of a true sentence. A `failed` run is
        // terminal *and* resumable, so its worktree is the one most likely to hold edits past the
        // last boundary, and nothing afterwards can say they were there. Not a refusal: whoever
        // typed `offload rm` is entitled to the disk back. What they are owed is being told.
        //
        // `commits_ahead` is deliberately left at zero — `parse_porcelain` does not set it — so
        // this counts only what teardown destroys, and never the commits it keeps.
        let discarded = git(
            Some(&path),
            &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
        )
        .await
        .ok()
        .map(|raw| crate::status::parse_porcelain(&raw))
        .filter(status::WorktreeStatus::is_dirty)
        .map(|status| status.summary());

        let removed = git(
            Some(&mirror),
            &["worktree", "remove", "--force", &path.display().to_string()],
        )
        .await;

        if let Err(e) = removed {
            // A worktree git has lost track of still occupies disk. Remove the directory
            // and let `prune` reconcile git's bookkeeping.
            tracing::warn!(error = %e, "git worktree remove failed; removing directory directly");
            std::fs::remove_dir_all(&path).map_err(|e| WorkspaceError::Io {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        }

        let _ = git(Some(&mirror), &["worktree", "prune"]).await;
        let _ = std::fs::remove_file(self.mgr.turn_marker(run));
        tracing::info!(run_id = %run, discarded = ?discarded, "workspace removed");
        Ok(Removal::Removed { discarded })
    }
}

/// Run a git command and return its stdout as bytes.
///
/// Bytes rather than a `String`, because git bundles and binary patches are not UTF-8 and
/// lossy conversion would silently corrupt them.
pub(crate) async fn git_bytes(
    dir: Option<&Path>,
    args: &[&str],
) -> Result<Vec<u8>, WorkspaceError> {
    let mut cmd = tokio::process::Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "true");

    let out = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            WorkspaceError::GitMissing
        } else {
            WorkspaceError::Io {
                path: dir.unwrap_or(Path::new(".")).to_path_buf(),
                reason: e.to_string(),
            }
        }
    })?;

    if out.status.success() {
        return Ok(out.stdout);
    }
    Err(WorkspaceError::Git {
        args: args.join(" "),
        code: out
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string()),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

/// Run a git command and return its stdout.
/// What an archive turned out to contain, for the run's own log (ADR-0061).
///
/// The mitigation the ADR names twice, and the §4 amendment promoted from a nicety to the thing
/// that makes a failure diagnosable. What travels is a **subset an agent chose**, so the failure
/// this design has is a run dying for want of a file that is sitting on the operator's disk — and
/// the only way to tell that apart from a broken workspace is to know what *was* there.
///
/// Top-level entries rather than every path: an agent selecting 12 MB out of 6.5 GB may still
/// bring thousands of files, and a log line nobody reads to the end reports nothing. The roots are
/// what the agent actually *chose*, which is the decision under review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// Files, not directories — the number somebody compares against what they expected to send.
    pub files: u64,
    /// Bytes on disk after unpacking, which is not the archive's own size.
    pub bytes: u64,
    /// The entries at the top of the tree, sorted, and whether the list was cut short.
    pub roots: Vec<String>,
    pub more_roots: usize,
}

impl std::fmt::Display for Acquired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} file(s), {} bytes", self.files, self.bytes)?;
        if !self.roots.is_empty() {
            write!(f, " — {}", self.roots.join(", "))?;
            if self.more_roots > 0 {
                write!(f, " and {} more", self.more_roots)?;
            }
        }
        Ok(())
    }
}

/// Count what came out of an archive, without walking it twice.
///
/// Best effort by construction: this is a **report**, and a directory it cannot read is a line
/// with a smaller number in it rather than a run that fails to start. The same posture
/// `usage::from_transcript` takes, and for the same reason — the cost of a wrong answer here is a
/// number in a log, and the cost of an error is a workspace that will not materialise.
fn describe(root: &Path) -> Acquired {
    const SHOWN: usize = 8;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // The synthetic history is not part of what the agent sent, and counting it would
            // make the number disagree with the archive somebody built.
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => {
                    files += 1;
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
                Err(_) => {}
            }
        }
    }
    let mut roots: Vec<String> = std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name() != ".git")
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    roots.sort();
    let more_roots = roots.len().saturating_sub(SHOWN);
    roots.truncate(SHOWN);
    Acquired {
        files,
        bytes,
        roots,
        more_roots,
    }
}

/// Make `root` a repository if it is not one, and bare-clone it into `at` as a mirror whose
/// origin branches are under `refs/remotes/origin/*`. What an archive's unpacked tree and a
/// scratch workspace's empty one both go through (ADR-0061 §2, ADR-0072).
///
/// `message` is part of the synthetic commit's id, and so of the base a migrated run's checkpoint
/// names: each caller's is fixed for ever, and pinned by a test.
async fn mirror_from_tree(root: &Path, at: &Path, message: &str) -> Result<(), WorkspaceError> {
    if !source::is_git_repo(root) {
        // §2: the receiving node makes it a repository — and it must make the **same** one
        // every other node would, or the run cannot migrate.
        //
        // This is the clause that broke the first time it was walked on two daemons. A `git
        // init` plus a commit embeds the moment it ran and the identity of whoever ran it, so
        // alpha and bravo unpacking identical bytes produced different base commits, and the
        // peer that took a migrated run over reported `fatal: invalid reference <alpha's
        // base>`. The checkpoint was right and the bundle was right; the history they are
        // rooted in did not exist on the receiver.
        //
        // So the repository is content-addressed the way the archive is: fixed identity,
        // fixed dates, fixed message, and a pinned initial branch — the last because
        // `init.defaultBranch` is somebody's global config, and two nodes disagreeing about
        // `main` versus `master` is the same failure by a slower route. Given identical bytes
        // this is an identical commit id on every node, which is the property `prepare`, the
        // bundle and `restore` already assume of a repository.
        git(
            Some(root),
            &["init", "--quiet", "--initial-branch", ARCHIVE_BRANCH],
        )
        .await?;
        git(Some(root), &["add", "-A"]).await?;
        git_at_epoch(
            root,
            &[
                "-c",
                "user.name=offload",
                "-c",
                "user.email=offload@localhost",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                message,
            ],
        )
        .await?;
    }

    git(
        None,
        &[
            "clone",
            "--bare",
            "--quiet",
            &root.display().to_string(),
            &at.display().to_string(),
        ],
    )
    .await?;
    git(
        Some(at),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    )
    .await?;
    // While the unpacked tree still exists: `prepare` resolves its start point as
    // `refs/remotes/origin/<default branch>`, and after this the origin is deleted.
    git(Some(at), &["fetch", "--prune", "--quiet", "origin"]).await?;
    Ok(())
}

/// The message of a scratch workspace's one commit (ADR-0072). Part of its id, so never changed.
pub(crate) const SCRATCH_MESSAGE: &str = "an empty workspace";

/// A mirror of an empty repository at `at`: one empty commit, built from an empty tree exactly as
/// an archive with no `.git` is, so it is the same commit on every node and a scratch run
/// migrates like any other.
async fn build_scratch_mirror(at: &Path) -> Result<(), WorkspaceError> {
    let empty = at.with_extension("empty");
    if empty.exists() {
        std::fs::remove_dir_all(&empty).map_err(|e| WorkspaceError::Io {
            path: empty.clone(),
            reason: e.to_string(),
        })?;
    }
    std::fs::create_dir_all(&empty).map_err(|e| WorkspaceError::Io {
        path: empty.clone(),
        reason: e.to_string(),
    })?;
    let built = mirror_from_tree(&empty, at, SCRATCH_MESSAGE).await;
    let _ = std::fs::remove_dir_all(&empty);
    built
}

/// The branch an archive's synthetic history is created on.
///
/// Pinned rather than left to `init.defaultBranch`, which is whoever-set-the-machine-up's global
/// config: two nodes disagreeing about `main` versus `master` turns one archive into two
/// repositories, which is the same migration failure as a drifting commit id by a slower route.
pub(crate) const ARCHIVE_BRANCH: &str = "offload-archive";

/// The instant an archive's synthetic commit is dated at, on every node, for ever.
///
/// A commit id hashes its tree, its parents, its message **and its two timestamps**, so a
/// synthetic root commit dated `now` is a different commit on every machine that builds it. The
/// value means nothing and is never shown; what matters is that it is the same everywhere and
/// never changes — the same class of constant as `cache_key`'s hash, and pinned by a test.
pub(crate) const ARCHIVE_EPOCH: &str = "2000-01-01T00:00:00Z";

/// `git`, with both commit timestamps pinned, so what it builds is content-addressed.
pub(crate) async fn git_at_epoch(dir: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
    git_with(
        Some(dir),
        args,
        &[
            ("GIT_AUTHOR_DATE", ARCHIVE_EPOCH),
            ("GIT_COMMITTER_DATE", ARCHIVE_EPOCH),
        ],
    )
    .await
}

pub(crate) async fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, WorkspaceError> {
    git_with(dir, args, &[]).await
}

async fn git_with(
    dir: Option<&Path>,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<String, WorkspaceError> {
    let mut cmd = tokio::process::Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    // Git must never stop to ask a human for anything: this runs headless, and a prompt
    // would hang the run rather than fail it.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "true");

    let out = cmd.output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            WorkspaceError::GitMissing
        } else {
            WorkspaceError::Io {
                path: dir.unwrap_or(Path::new(".")).to_path_buf(),
                reason: e.to_string(),
            }
        }
    })?;

    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }

    Err(WorkspaceError::Git {
        args: args.join(" "),
        code: out
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string()),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that cleans itself up.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "offload-ws-{tag}-{}-{:?}",
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

    /// Build a real git repo with one commit. These tests drive actual git — worktree
    /// semantics are exactly the kind of thing that looks right in a mock and isn't.
    async fn source_repo(at: &Path) -> RepoSource {
        std::fs::create_dir_all(at).expect("mkdir");
        git(Some(at), &["init", "--quiet", "--initial-branch=main"])
            .await
            .expect("init");
        git(Some(at), &["config", "user.email", "test@offload.local"])
            .await
            .expect("config email");
        git(Some(at), &["config", "user.name", "Offload Test"])
            .await
            .expect("config name");
        std::fs::write(at.join("README.md"), "hello\n").expect("write");
        git(Some(at), &["add", "."]).await.expect("add");
        git(Some(at), &["commit", "--quiet", "-m", "initial"])
            .await
            .expect("commit");
        RepoSource::Local(at.to_path_buf())
    }

    /// Tar a directory the way an agent would, and answer with the source naming it.
    async fn archive_of(dir: &Path, into: &Path, hash_byte: u8) -> (RepoSource, PathBuf) {
        let tarball = into.join(format!("archive-{hash_byte}.tar"));
        let out = tokio::process::Command::new("tar")
            .arg("-cf")
            .arg(&tarball)
            .arg("-C")
            .arg(dir.parent().expect("parent"))
            .arg(dir.file_name().expect("name"))
            .output()
            .await
            .expect("tar");
        assert!(out.status.success(), "tar failed");
        (
            RepoSource::Archive(offload_core::BlobHash::from_bytes([hash_byte; 32])),
            tarball,
        )
    }

    #[tokio::test]
    async fn an_archive_that_is_not_a_repository_becomes_one_on_the_receiver() {
        // ADR-0061 §2, the load-bearing clause. What arrives is bytes an agent chose; what this
        // leaves is an ordinary mirror, so everything downstream applies unchanged. The case is
        // the one the whole ADR is about: a directory that is not a git repository and has no
        // origin anybody could clone.
        let scratch = Scratch::new("archive-plain");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let tree = scratch.0.join("work/notes");
        std::fs::create_dir_all(tree.join("deep")).expect("mkdir");
        std::fs::write(tree.join("CLAUDE.md"), "the operator's notes\n").expect("write");
        std::fs::write(tree.join("deep/runbook.md"), "step one\n").expect("write");
        assert!(!source::is_git_repo(&tree), "the premise: not a repository");

        let (source, tarball) = archive_of(&tree, &scratch.0, 1).await;

        // Before acquisition the ordinary path refuses by name rather than building nothing.
        assert!(matches!(
            mgr.ensure_mirror(&source).await,
            Err(WorkspaceError::ArchiveNotAcquired { .. })
        ));

        let (mirror, acquired) = mgr
            .ensure_archive_mirror(&source, &tarball)
            .await
            .expect("archive mirror");
        assert!(mirror.join("HEAD").is_file(), "an ordinary bare mirror");

        // The report the §4 amendment promoted from a nicety: what travelled, so a run that
        // dies for want of a file can be told from a workspace that never arrived.
        let acquired = acquired.expect("the bytes moved now, so there is something to report");
        assert_eq!(acquired.files, 2, "{acquired}");
        assert_eq!(acquired.roots, vec!["CLAUDE.md", "deep/"], "{acquired}");
        assert!(acquired.to_string().contains("CLAUDE.md"), "{acquired}");

        // And now the ordinary path serves it, which is the whole point of §2.
        assert_eq!(mgr.ensure_mirror(&source).await.expect("mirror"), mirror);
        assert!(mgr.is_warm(&source), "warm once it is here");

        // A real worktree, a base commit and a run branch — nothing about `prepare` knows this
        // workspace arrived as bytes.
        let ws = mgr
            .hold(RunId::from_bytes([41; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        assert_eq!(
            std::fs::read_to_string(ws.path.join("CLAUDE.md")).expect("read"),
            "the operator's notes\n"
        );
        assert_eq!(
            std::fs::read_to_string(ws.path.join("deep/runbook.md")).expect("read"),
            "step one\n"
        );
        assert!(!ws.base_commit.is_empty(), "there is a base to bundle from");
    }

    #[tokio::test]
    async fn an_archive_that_is_already_a_repository_keeps_its_history() {
        // The other shape: `client-plugins` is 4.5 MB of history against a 3.6 GB tree, and the
        // history is the part worth carrying. If this re-initialised, the agent would arrive at
        // a repository with one commit and no past.
        let scratch = Scratch::new("archive-repo");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let tree = scratch.0.join("work/plugins");
        source_repo(&tree).await;
        std::fs::write(tree.join("second.txt"), "more\n").expect("write");
        git(Some(&tree), &["add", "."]).await.expect("add");
        git(Some(&tree), &["commit", "--quiet", "-m", "second"])
            .await
            .expect("commit");

        let (source, tarball) = archive_of(&tree, &scratch.0, 2).await;
        mgr.ensure_archive_mirror(&source, &tarball)
            .await
            .expect("archive mirror");

        let ws = mgr
            .hold(RunId::from_bytes([42; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        let log = git(Some(&ws.path), &["log", "--oneline"])
            .await
            .expect("log");
        assert_eq!(
            log.lines().count(),
            2,
            "both commits survived the archive: {log}"
        );
        assert!(ws.path.join("second.txt").is_file());
    }

    /// ADR-0075: a run's workspace, read-only — held to its root, `.git` hidden, a binary file
    /// shown as its size, and the committed files still shown once the checkout is gone.
    #[tokio::test]
    async fn a_workspace_is_shown_read_only_and_held_to_its_root() {
        use crate::peek::Shown;
        let scratch = Scratch::new("peek");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([61; 16]);
        let source = RepoSource::Scratch;
        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::create_dir_all(ws.path.join("notes")).expect("mkdir");
        std::fs::write(ws.path.join("notes/plan.md"), "step one\n").expect("write");
        std::fs::write(ws.path.join("blob.bin"), [0u8, 1, 2, 3]).expect("write");
        std::os::unix::fs::symlink("/etc", ws.path.join("escape")).expect("symlink");
        let branch = run_branch(run);

        let root = mgr.peek(run, &source, &branch, "").await.expect("root");
        assert!(root.from_checkout);
        let Shown::Listing(entries, _) = root.shown else {
            panic!("a listing")
        };
        let names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            names.first(),
            Some(&"notes"),
            "directories first: {names:?}"
        );
        assert!(!names.contains(&".git"), "{names:?}");

        let plan = mgr
            .peek(run, &source, &branch, "notes/plan.md")
            .await
            .expect("file");
        assert_eq!(plan.shown, Shown::Text("step one\n".into(), 9, false));
        assert_eq!(
            mgr.peek(run, &source, &branch, "blob.bin")
                .await
                .expect("bin")
                .shown,
            Shown::Binary(4)
        );
        for outside in ["../state", "/etc/passwd", "escape/passwd", ".git/config"] {
            assert!(
                mgr.peek(run, &source, &branch, outside).await.is_err(),
                "{outside} refused"
            );
        }

        // Committed, then the checkout goes: the branch in the mirror still shows it.
        git(Some(&ws.path), &["add", "notes"]).await.expect("add");
        git_at_epoch(
            &ws.path,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "plan",
            ],
        )
        .await
        .expect("commit");
        std::fs::remove_dir_all(&ws.path).expect("remove checkout");
        let later = mgr
            .peek(run, &source, &branch, "notes/plan.md")
            .await
            .expect("from the branch");
        assert!(!later.from_checkout);
        assert_eq!(later.shown, Shown::Text("step one\n".into(), 9, false));
        assert!(
            mgr.peek(run, &source, &branch, "blob.bin").await.is_err(),
            "uncommitted work went with the checkout"
        );
    }

    /// ADR-0074: checkouts move to a directory a person reads, and one made before the move is
    /// found where it was — adopted and reclaimed there, never rebuilt beside itself.
    #[tokio::test]
    async fn a_checkout_made_before_the_move_is_found_where_it_was() {
        let scratch = Scratch::new("checkouts-moved");
        let root = scratch.0.join("state");
        let before = WorkspaceManager::new(root.clone());
        let old_run = RunId::from_bytes([51; 16]);
        let old = before
            .hold(old_run)
            .await
            .prepare(&RepoSource::Scratch, None, None)
            .await
            .expect("prepare before");
        assert!(old.path.starts_with(root.join("worktrees")));
        before.note_turn(old_run, 3);

        let readable = scratch.0.join("offload");
        let after = WorkspaceManager::with_checkouts(root.clone(), readable.clone());
        let new_run = RunId::from_bytes([52; 16]);
        let new = after
            .hold(new_run)
            .await
            .prepare(&RepoSource::Scratch, None, None)
            .await
            .expect("prepare after");
        assert!(new.path.starts_with(&readable), "{}", new.path.display());
        assert_eq!(
            after.worktree_path(old_run),
            old.path,
            "the old one where it was"
        );
        assert_eq!(
            after.worktree_turn(old_run),
            Some(3),
            "with its turn marker"
        );
        let mut listed = after.checkouts();
        listed.sort_unstable();
        assert_eq!(listed, vec![old_run, new_run], "both are this node's");
        // Nothing but checkouts in the directory a person reads.
        let names: Vec<_> = std::fs::read_dir(&readable)
            .expect("readable")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![new_run.to_string()], "{names:?}");
    }

    /// ADR-0072: an agent run with no repository gets an empty one, made on the node that takes
    /// it — the same commit on every node, so it migrates, and pinned for the archive's reason.
    #[tokio::test]
    async fn a_scratch_workspace_is_the_same_empty_commit_everywhere_and_prepares() {
        let scratch = Scratch::new("scratch-workspace");
        let source = RepoSource::Scratch;
        let mut bases = Vec::new();
        for node in ["alpha", "bravo"] {
            let mgr = WorkspaceManager::new(scratch.0.join(node));
            assert!(mgr.reach(&source).is_obtainable());
            assert!(!mgr.is_warm(&source));
            let mirror = mgr.ensure_mirror(&source).await.expect("scratch mirror");
            // Warm now, and never refreshed: there is no origin to fetch from.
            assert!(mgr.is_warm(&source));
            assert_eq!(mgr.ensure_mirror(&source).await.expect("again"), mirror);
            let head = git(Some(&mirror), &["rev-parse", "HEAD"])
                .await
                .expect("rev-parse");
            bases.push(head.trim().to_string());

            let ws = mgr
                .hold(RunId::from_bytes([43; 16]))
                .await
                .prepare(&source, None, None)
                .await
                .expect("prepare");
            let listed = std::fs::read_dir(&ws.path)
                .expect("worktree")
                .filter_map(Result::ok)
                .filter(|e| e.file_name() != ".git")
                .count();
            assert_eq!(listed, 0, "an empty workspace, on its own run branch");
        }
        assert_eq!(bases[0], bases[1], "one empty commit on every node");
        assert_eq!(
            bases[0], "a869ccfe962f64bbd1057047bffeef421ca12d8a",
            "the scratch base commit is a wire format in all but name"
        );
    }

    #[tokio::test]
    async fn two_nodes_unpacking_one_archive_build_the_same_history() {
        // The property migration rests on, and the one the first two-daemon walk of ADR-0061
        // found missing. A `git init` plus a commit embeds the moment it ran, so alpha and bravo
        // unpacking identical bytes produced different base commits — and the peer that took a
        // migrated run over answered `fatal: invalid reference <alpha's base>`, with a correct
        // checkpoint and a correct bundle rooted in a history that did not exist there.
        //
        // Two managers under two state directories is two nodes for everything that matters
        // here: nothing about the result may depend on which machine did the unpacking.
        let scratch = Scratch::new("archive-deterministic");
        let tree = scratch.0.join("work/notes");
        std::fs::create_dir_all(tree.join("docs")).expect("mkdir");
        std::fs::write(tree.join("CLAUDE.md"), "notes\n").expect("write");
        std::fs::write(tree.join("docs/r.md"), "runbook\n").expect("write");
        let (source, tarball) = archive_of(&tree, &scratch.0, 9).await;

        let mut bases = Vec::new();
        for node in ["alpha", "bravo"] {
            let mgr = WorkspaceManager::new(scratch.0.join(node));
            let (mirror, _) = mgr
                .ensure_archive_mirror(&source, &tarball)
                .await
                .expect("archive mirror");
            let head = git(Some(&mirror), &["rev-parse", "HEAD"])
                .await
                .expect("rev-parse");
            bases.push(head.trim().to_string());
        }
        assert_eq!(
            bases[0], bases[1],
            "the same bytes must be the same history on every node"
        );

        // And pinned, because it is a value two *releases* have to agree on as well: a node that
        // upgraded mid-run must still find the base its peer's checkpoint names.
        assert_eq!(
            bases[0], "ed122b8d00a78baee4d5a1e9eaf7e6c7e6e58f8a",
            "the synthetic base commit is a wire format in all but name"
        );
    }

    #[tokio::test]
    async fn unpacking_the_same_archive_twice_is_the_same_mirror() {
        // Content-addressed, so the second call has nothing to do — and an archive's mirror is
        // never refreshed, because the origin it was built from is a scratch directory that no
        // longer exists and its name is the hash of its own bytes.
        let scratch = Scratch::new("archive-twice");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let tree = scratch.0.join("work/notes");
        std::fs::create_dir_all(&tree).expect("mkdir");
        std::fs::write(tree.join("a.txt"), "a\n").expect("write");

        let (source, tarball) = archive_of(&tree, &scratch.0, 3).await;
        let (first, reported) = mgr
            .ensure_archive_mirror(&source, &tarball)
            .await
            .expect("first");
        assert!(reported.is_some(), "the bytes moved");
        let (second, again) = mgr
            .ensure_archive_mirror(&source, &tarball)
            .await
            .expect("second");
        assert_eq!(first, second);
        // Nothing travelled the second time, so nothing is reported — a run must not log an
        // acquisition that did not happen, which is §3 read from the reporting side.
        assert!(again.is_none(), "the second call moved no bytes");
        // The refresh a `Local` mirror would do here would fail every time and say so.
        assert_eq!(mgr.ensure_mirror(&source).await.expect("third"), first);
    }

    #[tokio::test]
    async fn a_local_path_is_obtainable_only_where_it_exists() {
        // The eligibility fact behind roadmap #4: a run whose repo is a path can be placed on
        // the machines holding that path and nowhere else, and finding that out at resume
        // time means the migration has already failed.
        let scratch = Scratch::new("obtain");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let repo = source_repo(&scratch.0.join("src")).await;

        assert_eq!(
            mgr.reach(&repo),
            RepoReach::Obtainable,
            "the path is right there"
        );
        assert_eq!(
            mgr.reach(&RepoSource::parse(
                &scratch.0.join("nowhere").display().to_string()
            )),
            RepoReach::NotHere
        );
        assert_eq!(
            mgr.reach(&RepoSource::parse("https://example.com/me/api.git")),
            RepoReach::Obtainable,
            "any node can fetch a URL, which is what fleetwide means"
        );

        // The two causes the refusal used to say one sentence about. A directory sitting right
        // here that is not a repository was reported as "a path on another machine" by the
        // machine it was on, because the message was derived from the string and not from a
        // look at the disk.
        let plain = scratch.0.join("just-a-directory");
        std::fs::create_dir_all(&plain).expect("mkdir");
        std::fs::write(plain.join("notes.txt"), "hello\n").expect("write");
        assert_eq!(
            mgr.reach(&RepoSource::Local(plain)),
            RepoReach::NotARepo,
            "here, and nothing to clone — which is not the same refusal"
        );

        // A task names no repo at all, and `""` is not a directory on any machine.
        assert_eq!(mgr.reach(&RepoSource::parse("")), RepoReach::NotHere);

        // Once mirrored, the path is obtainable here even if the original goes away.
        mgr.ensure_mirror(&repo).await.expect("mirror");
        std::fs::remove_dir_all(scratch.0.join("src")).expect("remove the original");
        assert_eq!(mgr.reach(&repo), RepoReach::Obtainable);
    }

    #[tokio::test]
    async fn prepare_gives_the_run_its_own_branch_and_checkout() {
        let scratch = Scratch::new("prepare");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([1; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        assert!(ws.path.join("README.md").is_file(), "checkout is populated");
        assert_eq!(ws.branch, run_branch(run));
        assert_eq!(ws.base_commit.len(), 40, "base is a full sha");

        let branch = git(Some(&ws.path), &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .expect("rev-parse");
        assert_eq!(branch.trim(), run_branch(run));
    }

    #[tokio::test]
    async fn a_second_run_on_the_same_repo_reuses_the_mirror() {
        // The basis of the workspace_warm bid signal: the mirror is per-repo, not per-run.
        let scratch = Scratch::new("warm");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        assert!(!mgr.is_warm(&source), "cold before the first run");

        let a = mgr
            .hold(RunId::from_bytes([1; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("first");
        assert!(mgr.is_warm(&source), "warm once the mirror exists");

        let b = mgr
            .hold(RunId::from_bytes([2; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("second");

        assert_ne!(a.path, b.path, "runs get separate worktrees");
        assert_ne!(a.branch, b.branch, "runs get separate branches");
        assert_eq!(
            std::fs::read_dir(mgr.repos_dir())
                .expect("read repos")
                .count(),
            1,
            "one mirror shared by both runs"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_runs_starting_at_once_on_a_cold_repo_all_get_a_workspace() {
        // Found by submitting three runs for one repo in the same second: `git clone` into
        // the final path is not atomic, so the two that lost the race died on `destination
        // path already exists` — and left a directory with no `HEAD`, which every later run
        // on that repo would then try to clone over, failing identically for ever.
        let scratch = Scratch::new("cold-race");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        let mut starting = Vec::new();
        for i in 1..=3u8 {
            let mgr = mgr.clone();
            let source = source.clone();
            starting.push(tokio::spawn(async move {
                mgr.hold(RunId::from_bytes([i; 16]))
                    .await
                    .prepare(&source, None, None)
                    .await
            }));
        }

        for (i, started) in starting.into_iter().enumerate() {
            let workspace = started
                .await
                .expect("task")
                .unwrap_or_else(|e| panic!("run {i} lost the race: {e}"));
            assert!(workspace.path.join("README.md").is_file());
        }

        // One mirror, and no half-built leftovers beside it.
        let mirrors: Vec<String> = std::fs::read_dir(mgr.repos_dir())
            .expect("read repos")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(mirrors.len(), 1, "expected one mirror, found {mirrors:?}");
        assert!(mgr.is_warm(&source));
    }

    #[tokio::test]
    async fn fetching_does_not_delete_run_branches() {
        // The bug a --mirror clone would have: its +refs/*:refs/* refspec means
        // `fetch --prune` deletes every offload/run-* branch, because none exist
        // upstream. That would destroy the agent's commits.
        let scratch = Scratch::new("refspec");
        let src = scratch.0.join("src");
        let source = source_repo(&src).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([3; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(ws.path.join("work.txt"), "agent output\n").expect("write");
        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "agent work"])
            .await
            .expect("commit");

        // Something changes upstream, and we refresh.
        std::fs::write(src.join("other.txt"), "upstream\n").expect("write");
        git(Some(&src), &["add", "."]).await.expect("add");
        git(Some(&src), &["commit", "--quiet", "-m", "upstream change"])
            .await
            .expect("commit");
        mgr.ensure_mirror(&source).await.expect("refresh");

        let branches = git(
            Some(&mgr.mirror_path(&source)),
            &["branch", "--list", &run_branch(run)],
        )
        .await
        .expect("branch list");
        assert!(
            branches.contains(&run_branch(run)),
            "run branch survived a prune-fetch, got {branches:?}"
        );
        assert!(
            ws.path.join("work.txt").is_file(),
            "agent's work still there"
        );
    }

    #[tokio::test]
    async fn status_sees_what_the_agent_did() {
        let scratch = Scratch::new("status");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let ws = mgr
            .hold(RunId::from_bytes([4; 16]))
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        assert!(!mgr.status(&ws).await.expect("status").has_work());

        std::fs::write(ws.path.join("README.md"), "changed\n").expect("modify");
        std::fs::write(ws.path.join("new.rs"), "fn main() {}\n").expect("create");

        let status = mgr.status(&ws).await.expect("status");
        assert_eq!(status.modified, vec!["README.md"]);
        assert_eq!(
            status.untracked,
            vec!["new.rs"],
            "an untracked new file is work; dropping it loses what the agent wrote"
        );
        assert_eq!(status.commits_ahead, 0);

        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "work"])
            .await
            .expect("commit");

        let status = mgr.status(&ws).await.expect("status");
        assert!(!status.is_dirty());
        assert_eq!(status.commits_ahead, 1);
        assert!(status.has_work(), "committed work still needs migrating");
    }

    /// What the disk holds, asked of the disk (ADR-0023).
    ///
    /// The list a reclaim sweep works from, and it has to be the *filesystem's* answer rather
    /// than the store's: the checkouts worth reclaiming are precisely the ones no record points
    /// at any more. Turn markers live beside the worktrees rather than inside them, so this also
    /// pins that they are not mistaken for one.
    #[tokio::test]
    async fn the_checkouts_on_this_machine_are_asked_of_the_disk() {
        let scratch = Scratch::new("checkouts");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));

        assert!(mgr.checkouts().is_empty(), "nothing prepared yet");

        let one = RunId::from_bytes([1; 16]);
        let two = RunId::from_bytes([2; 16]);
        for run in [one, two] {
            mgr.hold(run)
                .await
                .prepare(&source, None, None)
                .await
                .expect("prepare");
            mgr.note_turn(run, 3);
        }

        let mut found = mgr.checkouts();
        found.sort();
        let mut want = vec![one, two];
        want.sort();
        assert_eq!(found, want, "both, and neither turn marker");

        mgr.hold(one).await.remove(&source).await.expect("remove");
        assert_eq!(mgr.checkouts(), vec![two], "and it follows the disk");
    }

    #[tokio::test]
    async fn remove_says_what_the_teardown_destroyed() {
        // `offload rm` answered "worktree removed (the run's branch and commits are kept)" and
        // nothing else, whatever the checkout held — the reassuring half of a true sentence, and
        // the only half. A `failed` run is terminal *and* resumable, so its worktree is the one
        // most likely to hold edits past the last turn boundary; measured on two daemons, a
        // modified file and an untracked one went, the run then resumed to completion, and
        // `offload ps`, `offload logs` and `offload audit` between them said nothing about
        // either. Asked before the teardown, because afterwards there is nothing left to ask.
        let scratch = Scratch::new("remove-reports");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([7; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");

        // Committed work is on the run branch and survives, so it is not what this reports —
        // `commits_ahead` stays out of the count deliberately.
        std::fs::write(
            ws.path.join("committed.txt"),
            "safe
",
        )
        .expect("write");
        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "work"])
            .await
            .expect("commit");
        // …and this is the part nothing anywhere else has a copy of.
        std::fs::write(
            ws.path.join("committed.txt"),
            "edited
",
        )
        .expect("write");
        std::fs::write(
            ws.path.join("never-saved.txt"),
            "the only copy
",
        )
        .expect("write");

        let removal = mgr.hold(run).await.remove(&source).await.expect("remove");
        let Removal::Removed {
            discarded: Some(held),
        } = removal
        else {
            panic!("a checkout holding uncommitted work reported nothing: {removal:?}");
        };
        assert_eq!(
            held, "1 modified, 1 new",
            "the count is of what teardown destroyed, and the commit is not part of that"
        );
    }

    #[tokio::test]
    async fn a_clean_teardown_has_nothing_to_report() {
        // The other half, and the one that keeps the sentence from crying wolf: the ordinary
        // `offload rm` is on a completed run whose agent committed everything, and a line about
        // losing work there would be noise on every teardown in the fleet.
        let scratch = Scratch::new("remove-clean");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([8; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(
            ws.path.join("work.txt"),
            "x
",
        )
        .expect("write");
        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "work"])
            .await
            .expect("commit");

        assert_eq!(
            mgr.hold(run).await.remove(&source).await.expect("remove"),
            Removal::Removed { discarded: None },
            "a checkout with every change committed has nothing to say it lost"
        );
    }

    #[tokio::test]
    async fn remove_deletes_the_worktree_but_keeps_the_agents_commits() {
        // Teardown must not discard work a checkpoint may still need to bundle.
        let scratch = Scratch::new("remove");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([5; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(ws.path.join("work.txt"), "x\n").expect("write");
        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "work"])
            .await
            .expect("commit");

        mgr.hold(run).await.remove(&source).await.expect("remove");
        assert!(!ws.path.exists(), "worktree directory gone");

        let branches = git(
            Some(&mgr.mirror_path(&source)),
            &["branch", "--list", &run_branch(run)],
        )
        .await
        .expect("branch list");
        assert!(branches.contains(&run_branch(run)), "commits preserved");
    }

    /// The question a trigger's rule asks before it reclaims a spent occurrence's checkout
    /// (ADR-0020), and it has to give three different answers.
    ///
    /// `None` — not here — is the one that is easy to collapse into `Some(false)` and must not
    /// be: a node that read a missing worktree as "nothing to lose" would be deciding about a
    /// machine it cannot see. And committed work is deliberately *not* counted, because the
    /// branch survives teardown and the fragile part is the part that does not.
    #[tokio::test]
    async fn holds_uncommitted_separates_not_here_from_nothing_here() {
        let scratch = Scratch::new("spent");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([11; 16]);

        assert_eq!(
            mgr.holds_uncommitted(run).await,
            None,
            "a checkout that is not on this node is not an empty one"
        );

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        assert_eq!(
            mgr.holds_uncommitted(run).await,
            Some(false),
            "a fresh checkout holds nothing"
        );

        // An untracked file counts. A newly written source file is exactly what an agent
        // produces and exactly what `git status` without --untracked-files=all would miss.
        std::fs::write(ws.path.join("new.txt"), "x\n").expect("write");
        assert_eq!(mgr.holds_uncommitted(run).await, Some(true));

        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "work"])
            .await
            .expect("commit");
        assert_eq!(
            mgr.holds_uncommitted(run).await,
            Some(false),
            "committed work is on the branch and survives teardown, so it does not hold the \
             checkout here — `remove` says so in as many words"
        );
    }

    #[tokio::test]
    async fn a_surviving_worktree_is_adopted_rather_than_rebuilt() {
        // Resume's first question. The checkout outlives the daemon, and while the run never
        // left this node its contents are never staler than the last checkpoint —
        // rebuilding from blobs would go backwards.
        let scratch = Scratch::new("adopt");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([7; 16]);

        assert_eq!(
            mgr.hold(run).await.adopt(&run_branch(run), "deadbeef", 0),
            Adoption::Absent,
            "nothing to adopt before the run has been prepared"
        );

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(ws.path.join("wip.rs"), "fn half(\n").expect("write");
        // What a capture on this node records: the checkout holds turn 4's work.
        mgr.note_turn(run, 4);

        let Adoption::Current(adopted) = mgr.hold(run).await.adopt(&ws.branch, &ws.base_commit, 4)
        else {
            panic!("the checkout that produced this checkpoint is the one to use");
        };
        assert_eq!(adopted, ws);
        assert!(
            adopted.path.join("wip.rs").is_file(),
            "with the uncommitted work still in it"
        );

        assert!(mgr.has_branch(&source, &ws.branch).await);
        assert!(!mgr.has_branch(&source, "offload/run-nonexistent").await);
    }

    #[tokio::test]
    async fn a_checkout_from_an_earlier_leg_is_moved_aside_rather_than_adopted_or_deleted() {
        // The run left, did more turns elsewhere, and came back. Nothing removes a worktree
        // when a run leaves, so this checkout is still here and still holds turn 1 — and the
        // checkpoint being resumed holds turn 9. Adopting it would resume the agent into a
        // filesystem from before everything the other machine did.
        let scratch = Scratch::new("legs");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([21; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        std::fs::write(ws.path.join("mid_turn.rs"), "fn never_captured(\n").expect("write");
        mgr.note_turn(run, 1);

        assert_eq!(
            mgr.hold(run).await.adopt(&ws.branch, &ws.base_commit, 9),
            Adoption::Superseded { at_turn: Some(1) },
        );

        let Rescued::MovedAside(moved) = mgr
            .hold(run)
            .await
            .supersede(&source)
            .await
            .expect("supersede")
        else {
            panic!("a checkout holding an uncaptured file is the case the rescue exists for");
        };
        assert!(
            moved.join("mid_turn.rs").is_file(),
            "the rescued copy keeps what was never captured: {}",
            moved.display()
        );
        assert!(!ws.path.exists(), "and the worktree path is free again");
        assert_eq!(
            mgr.hold(run).await.adopt(&ws.branch, &ws.base_commit, 9),
            Adoption::Absent,
            "so the resume rebuilds from the checkpoint"
        );

        // The whole point of the rename: the run's branch is no longer checked out anywhere,
        // so the rebuild can have it. Git refuses a second worktree on one branch.
        let rebuilt = mgr
            .hold(run)
            .await
            .prepare(&source, Some(&ws.base_commit), Some(&ws.branch))
            .await
            .expect("prepare over the top");
        assert_eq!(rebuilt.path, ws.path);
        assert!(!rebuilt.path.join("mid_turn.rs").exists());
    }

    #[tokio::test]
    async fn a_worktree_that_never_said_which_turn_it_holds_is_not_assumed_current() {
        // A run in flight across an upgrade: the checkout is here and nothing recorded what
        // it holds. The convenient reading is "it must be ours"; the safe one is to rebuild
        // from the checkpoint and keep the old copy, because unknown is not current.
        let scratch = Scratch::new("unmarked");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([22; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        assert_eq!(
            mgr.hold(run).await.adopt(&ws.branch, &ws.base_commit, 3),
            Adoption::Superseded { at_turn: None },
        );

        // Two rescues, because a run can come back more than once — and the second must not
        // land on top of the first. Each leg is made dirty first: an unknown turn is a reason
        // not to *adopt* a checkout, and what is in it is a separate question with its own
        // answer (below), so a clean one would be removed here and never reach the naming.
        std::fs::write(ws.path.join("leg1.rs"), "fn one(\n").expect("write");
        let Rescued::MovedAside(first) =
            mgr.hold(run).await.supersede(&source).await.expect("first")
        else {
            panic!("it holds an uncommitted file");
        };
        let again = mgr
            .hold(run)
            .await
            .prepare(&source, Some(&ws.base_commit), Some(&ws.branch))
            .await
            .expect("prepare again");
        std::fs::write(again.path.join("leg2.rs"), "fn two(\n").expect("write");
        let Rescued::MovedAside(second) = mgr
            .hold(run)
            .await
            .supersede(&source)
            .await
            .expect("second")
        else {
            panic!("and so does this one");
        };
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert!(
            first.join("leg1.rs").is_file() && second.join("leg2.rs").is_file(),
            "and each rescue kept its own leg's work rather than the other's"
        );
    }

    #[tokio::test]
    async fn a_superseded_checkout_holding_nothing_uncommitted_is_removed_rather_than_kept_for_ever(
    ) {
        // **The rescue asks what it is rescuing.** For two phases `supersede` renamed
        // unconditionally, which sounds like the cautious choice and is not: the rename deletes
        // the copy's `.git` file, so from that moment nothing can tell whether the directory held
        // anything the mirror did not — measured, `git status` in a rescued copy answers `fatal:
        // not a git repository`. Every rescue was therefore permanent and indistinguishable from
        // the one that mattered, and nothing in the product ever removed one.
        //
        // A checkout with nothing uncommitted is redundant by *measurement* rather than by
        // judgement: its commits are on the run branch in the mirror, which `remove` keeps on
        // purpose, and that is asserted here rather than argued.
        let scratch = Scratch::new("redundant");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([24; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        git(Some(&ws.path), &["config", "user.email", "t@offload.local"])
            .await
            .expect("email");
        git(Some(&ws.path), &["config", "user.name", "Offload Test"])
            .await
            .expect("name");
        std::fs::write(ws.path.join("committed.rs"), "fn kept() {}\n").expect("write");
        git(Some(&ws.path), &["add", "."]).await.expect("add");
        git(Some(&ws.path), &["commit", "--quiet", "-m", "leg one"])
            .await
            .expect("commit");
        mgr.note_turn(run, 1);

        // The staging is the same as the rescue's: this leg is behind the checkpoint being
        // resumed. Only the contents differ, and the contents are what decides.
        assert_eq!(
            mgr.hold(run).await.adopt(&ws.branch, &ws.base_commit, 9),
            Adoption::Superseded { at_turn: Some(1) },
        );
        assert_eq!(
            mgr.hold(run)
                .await
                .supersede(&source)
                .await
                .expect("supersede"),
            Rescued::Redundant,
        );

        assert!(
            !ws.path.exists(),
            "the worktree path is free for the rebuild"
        );
        let strays: Vec<_> = std::fs::read_dir(mgr.worktrees_dir())
            .expect("read worktrees dir")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .filter(|name| name.contains("superseded"))
            .collect();
        assert!(
            strays.is_empty(),
            "and no copy of it was kept for ever: {strays:?}"
        );

        // The claim `remove` makes and this leans on: the commits are not what was thrown away.
        let branches = git(
            Some(&mgr.mirror_path(&source)),
            &["branch", "--list", "--format=%(refname:short)"],
        )
        .await
        .expect("branch --list");
        assert!(
            branches.lines().any(|b| b.trim() == ws.branch),
            "the run branch still holds the leg's commit: {branches}"
        );
    }

    #[tokio::test]
    async fn a_superseded_checkout_git_will_not_talk_about_is_kept() {
        // Unknown is not nothing, at the one door here whose wrong answer deletes somebody's
        // only copy of a file. `holds_uncommitted` answers `None` for a worktree it could not
        // read, and the rescue must take that as a reason to keep rather than as a clean bill —
        // staged by removing the `.git` file that makes the question answerable, which is
        // precisely what the rename itself does one line later.
        let scratch = Scratch::new("unreadable");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([25; 16]);

        let ws = mgr
            .hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("prepare");
        mgr.note_turn(run, 1);
        std::fs::remove_file(ws.path.join(".git")).expect("break the link");
        assert_eq!(
            mgr.holds_uncommitted(run).await,
            None,
            "the staging: git will not answer about this directory"
        );

        let kept = mgr
            .hold(run)
            .await
            .supersede(&source)
            .await
            .expect("supersede");
        assert!(
            matches!(kept, Rescued::MovedAside(_)),
            "an unanswerable question keeps the copy, and this said {kept:?}"
        );
    }

    #[test]
    fn a_worktree_only_ever_moves_forward_through_the_turns() {
        // A restore of an older checkpoint does not make the checkout older than the work it
        // already holds, so the marker never goes backwards.
        let scratch = Scratch::new("turns");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([23; 16]);

        assert_eq!(mgr.worktree_turn(run), None);
        mgr.note_turn(run, 7);
        mgr.note_turn(run, 3);
        assert_eq!(mgr.worktree_turn(run), Some(7));
        mgr.note_turn(run, 8);
        assert_eq!(mgr.worktree_turn(run), Some(8));
    }

    #[tokio::test]
    async fn removing_a_run_that_was_never_prepared_is_not_an_error() {
        // Cleanup runs on paths that may have already been cleaned up.
        let scratch = Scratch::new("idempotent");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        mgr.hold(RunId::from_bytes([9; 16]))
            .await
            .remove(&source)
            .await
            .expect("remove of nothing succeeds");
    }

    #[tokio::test]
    async fn preparing_twice_recovers_instead_of_failing() {
        // After a crash the branch and directory may survive. A run that cannot restart
        // because of leftovers from its own previous attempt is a bad failure mode.
        let scratch = Scratch::new("recover");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let run = RunId::from_bytes([6; 16]);

        mgr.hold(run)
            .await
            .prepare(&source, None, None)
            .await
            .expect("first");
        mgr.hold(run).await.remove(&source).await.expect("remove");
        let again = mgr.hold(run).await.prepare(&source, None, None).await;
        assert!(again.is_ok(), "re-prepare failed: {:?}", again.err());
    }

    #[tokio::test]
    async fn a_non_repo_path_is_reported_clearly() {
        let scratch = Scratch::new("notrepo");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let not_a_repo = RepoSource::Local(scratch.0.join("empty"));
        std::fs::create_dir_all(scratch.0.join("empty")).expect("mkdir");

        assert!(matches!(
            mgr.ensure_mirror(&not_a_repo).await,
            Err(WorkspaceError::NotARepo(_))
        ));
    }

    #[test]
    fn a_checkout_is_held_by_one_caller_at_a_time() {
        // The sweep's door, without a runtime: `try_hold` is what lets it skip a checkout
        // somebody else is building rather than wait behind a bundle fetch. Two managers over
        // one root, because the production type is `Clone` and a lock a clone does not share
        // is not a lock — the map is behind an `Arc` for exactly this.
        let scratch = Scratch::new("hold");
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        let also = mgr.clone();
        let run = RunId::from_bytes([7; 16]);
        let other = RunId::from_bytes([8; 16]);

        let held = mgr.try_hold(run).expect("free to begin with");
        assert!(
            also.try_hold(run).is_none(),
            "a clone of the manager must see the same checkout held"
        );
        assert!(
            also.try_hold(other).is_some(),
            "the key is the checkout, not the manager: another run is unaffected"
        );
        drop(held);
        assert!(
            also.try_hold(run).is_some(),
            "dropping the guard releases the checkout"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_rebuild_never_adopts_a_checkout_a_teardown_is_removing() {
        // The forced pair behind ADR-0052, run across the window it used to lose. Before the
        // guard: 10 of 60 passes adopted a checkout that was gone by the time `adopt` returned,
        // and *every* pass starting 0-1ms into the removal did — `git worktree remove --force`
        // is about 2ms of somebody else's program, so a guard read before it is on the wrong
        // side of it. The invariant is not "the rebuild wins": it is that whatever the rebuild
        // says is usable is still on the disk afterwards.
        let scratch = Scratch::new("pair");
        let source = source_repo(&scratch.0.join("src")).await;
        let mgr = WorkspaceManager::new(scratch.0.join("state"));
        mgr.ensure_mirror(&source).await.expect("mirror");

        for delay in 0..4u8 {
            let run = RunId::from_bytes([delay + 1; 16]);
            let ws = mgr
                .hold(run)
                .await
                .prepare(&source, None, None)
                .await
                .expect("prepare");
            mgr.note_turn(run, 4);

            let teardown = {
                let (mgr, source) = (mgr.clone(), source.clone());
                tokio::spawn(async move { mgr.hold(run).await.remove(&source).await })
            };
            std::thread::sleep(std::time::Duration::from_millis(u64::from(delay)));
            let rebuild = {
                let (mgr, source) = (mgr.clone(), source.clone());
                let (branch, base) = (ws.branch.clone(), ws.base_commit.clone());
                tokio::spawn(async move {
                    let held = mgr.hold(run).await;
                    let (ws, how) = match held.adopt(&branch, &base, 4) {
                        Adoption::Current(found) => (found, "adopted"),
                        _ => (
                            held.prepare(&source, Some(&base), Some(&branch)).await?,
                            "rebuilt",
                        ),
                    };
                    // **Asked while the hold is still ours, which is the whole of what ADR-0052
                    // promises.** The guard's own doc says why it may be dropped at all: from
                    // there on the run is in `live` with an agent handle, and every teardown door
                    // asks about that. This staging has no such handle — its teardown is
                    // `remove` with nothing in front of it — so a rebuild that *wins* the lock,
                    // hands its checkout back and releases is asking to have it deleted, and the
                    // deletion is correct. Asserting after both tasks joined therefore tested an
                    // ordering nothing guarantees, and failed about 1 run in 5 under load with
                    // `rebuild adopted, teardown Removed`: the lock working exactly as designed.
                    //
                    // It still catches what it was written for. Without serialisation the
                    // removal overlaps this check rather than following it — 10 of 10 within the
                    // 2ms window, before the guard existed — and the directory goes out from
                    // under these two lines.
                    let intact =
                        ws.path.join(".git").exists() && ws.path.join("README.md").is_file();
                    Ok::<_, WorkspaceError>((how, intact))
                })
            };

            let removal = teardown.await.expect("teardown task").expect("removal");
            let (how, intact) = rebuild.await.expect("rebuild task").expect("rebuild");

            assert!(
                intact,
                "at delay={delay}ms the rebuild held a checkout that was not there, or was \
                 registered and empty (rebuild {how}, teardown {removal:?})"
            );
            // Both orderings are legal and both must remove exactly one checkout — the teardown
            // finding nothing would mean it had raced past the thing it was meant to tear down.
            assert!(
                matches!(removal, Removal::Removed { .. }),
                "at delay={delay}ms the teardown had nothing to remove (rebuild {how})"
            );
            mgr.hold(run).await.remove(&source).await.expect("tidy up");
        }
    }
}
