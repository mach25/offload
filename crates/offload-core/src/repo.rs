//! Where a repository comes from, and whether that answer survives leaving this machine.
//!
//! This is a smaller question than it looks and a more important one. A run whose repo is
//! `/home/owner/dev/api` can only be placed on the machine where that path means something.
//! A run whose repo is `git@github.com:me/api.git` can go anywhere. The scheduler needs to
//! know which it is *before* placing the run, because discovering it at resume time means the
//! migration has already failed — which is why this lives in `offload-core`, where bidding
//! can see it, rather than beside the code that does the cloning.

use crate::id::BlobHash;
use std::path::PathBuf;

/// The prefix that spells an archive workspace in a run spec (ADR-0061 §1).
///
/// `RunSpec::repo` is a `String` — it is stored and gossiped that way — so an archive has to be
/// sayable as one. A prefix rather than a new field, because every existing reader of that string
/// keeps working and a node too old to know the form refuses it as an unobtainable path rather
/// than misreading it as a directory.
pub const ARCHIVE_PREFIX: &str = "archive:";

/// The spelling of a scratch workspace in a run spec (ADR-0072): an empty repository the node
/// that takes the run makes for itself.
///
/// For the agent run that has no repository to work in, which from a phone is most of them. The
/// whole string, not a prefix: there is nothing to name. Spelled like `archive:` for the same
/// reason, so a node too old to know it refuses it as a path that is not here, never as a
/// directory it misreads.
pub const SCRATCH: &str = "scratch:";

/// A repository, as the run spec names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoSource {
    /// A path on this machine. Fast, but meaningless to any other node.
    Local(PathBuf),
    /// A URL any node with credentials can fetch.
    Remote(String),
    /// **These bytes** — a content-addressed archive on the blob plane (ADR-0061 §1).
    ///
    /// The workspace the submitting agent chose, rather than one every node can clone. It is
    /// `Fleetwide` for the reason a URL is: the hash is obtainable from any node that can reach
    /// a peer holding it, and the hash is the authority, exactly as it already is for every
    /// checkpoint. What makes this different from a URL is that nothing outside the fleet has
    /// to exist — which is the whole point, since the motivating workspace is a directory that
    /// is not a repository and has no origin to clone from.
    Archive(BlobHash),
    /// **Nothing** — an empty repository, made by whichever node takes the run (ADR-0072).
    ///
    /// Built the way an archive with no `.git` is (ADR-0061 §2), from an empty tree, so every
    /// node makes the same base commit and the run migrates like any other. There are no bytes
    /// to fetch and no origin to clone.
    Scratch,
}

/// How far a workspace can travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Portability {
    /// Any node can obtain this repo for itself.
    Fleetwide,
    /// Only nodes that already happen to hold this path can host the run.
    ///
    /// Not a hard block on migration — phase 3's git bundle carries the objects, so a
    /// receiving node can materialise the workspace without ever seeing the origin. But
    /// it does mean the run cannot be *placed* somewhere cold, and that is an eligibility
    /// fact, not a scoring one.
    NodeLocal,
}

/// Whether *this* node can get the repo, and if not, why not.
///
/// A `bool` here was right twice and confidently wrong once. `Portability` is a fact about the
/// repo *string* — anybody can derive it, anywhere — and a refusal computed from it says the
/// same sentence for two different machines' reasons. A local path refused because nothing of
/// that name is here and a local path refused because the directory is sitting right there and
/// is not a git repository are the same `NodeLocal`, and the operator was told *"is a path on
/// another machine"* by the machine they typed it on.
///
/// So this is measured rather than parsed: `WorkspaceManager::reach` looks at the filesystem,
/// and the answer travels to the refusal instead of being re-derived from the string beside it.
/// Fleetwide repos are always `Obtainable` — that is what fleetwide means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoReach {
    /// This node can get it: a clone URL, a path that is a repository here, or one already
    /// mirrored here.
    Obtainable,
    /// Nothing of that name is on this machine. The run belongs where the path does.
    NotHere,
    /// The path *is* on this machine and is not a git repository, so there is nothing to
    /// clone — the one refusal no other node could have worded differently. ADR-0061 is the
    /// decision about whether such a directory should be able to travel at all; until then
    /// this is the honest sentence.
    NotARepo,
}

impl RepoReach {
    /// Would a bid survive this?
    #[must_use]
    pub fn is_obtainable(self) -> bool {
        matches!(self, RepoReach::Obtainable)
    }
}

impl RepoSource {
    /// Classify a repo string the way git would.
    #[must_use]
    pub fn parse(repo: &str) -> Self {
        if repo == SCRATCH {
            return RepoSource::Scratch;
        }
        // Asked first, and by exact prefix: `archive:` cannot be a path anybody meant, and the
        // scp-style test below would otherwise claim it (it has an `@`-free colon, but a
        // 64-character hex tail is a plausible enough `host:path` that leaving the order to
        // chance is not worth it). A malformed digest falls through to `Local`, which refuses
        // it as a path that is not here — wrong-looking, and it is the safe direction: an
        // unreadable archive name must never be treated as an archive somebody can serve.
        if let Some(digest) = repo.strip_prefix(ARCHIVE_PREFIX) {
            if let Ok(hash) = BlobHash::parse_hex(digest) {
                return RepoSource::Archive(hash);
            }
        }
        let looks_remote = repo.contains("://")
            // scp-style: git@host:path — distinguish from a Windows drive letter by
            // requiring something before the colon that isn't a single letter.
            || (repo.contains('@') && repo.contains(':'));
        if looks_remote {
            RepoSource::Remote(repo.to_string())
        } else {
            RepoSource::Local(PathBuf::from(repo))
        }
    }

    #[must_use]
    pub fn portability(&self) -> Portability {
        match self {
            // An archive is fleetwide for the same reason a URL is, and it is the entire
            // headline of ADR-0061: a workspace that no node could clone can now migrate.
            RepoSource::Remote(_) | RepoSource::Archive(_) | RepoSource::Scratch => {
                Portability::Fleetwide
            }
            RepoSource::Local(_) => Portability::NodeLocal,
        }
    }

    /// The archive this workspace is, if it is one.
    #[must_use]
    pub fn archive(&self) -> Option<BlobHash> {
        match self {
            RepoSource::Archive(hash) => Some(*hash),
            _ => None,
        }
    }

    /// What to hand `git clone`.
    #[must_use]
    pub fn clone_target(&self) -> String {
        match self {
            RepoSource::Local(p) => p.display().to_string(),
            RepoSource::Remote(url) => url.clone(),
            // Nothing clones an archive — it is unpacked from bytes this node already holds
            // (ADR-0061 §2). This is the round-trip of `parse`, and it is what `cache_key`
            // hashes, so it has to stay exactly the spelling a run spec carries.
            RepoSource::Archive(hash) => format!("{ARCHIVE_PREFIX}{hash}"),
            RepoSource::Scratch => SCRATCH.to_string(),
        }
    }

    /// A stable, filesystem-safe directory name for this repo's local mirror.
    ///
    /// Readable prefix plus a hash, because basenames collide — `~/work/a/api` and
    /// `~/work/b/api` are different repos and must not share a mirror. The prefix exists
    /// purely so a human looking at the state directory can tell what is in it.
    #[must_use]
    pub fn cache_key(&self) -> String {
        // An archive's target is `archive:` plus 64 hex characters, and the generic path below
        // would make a directory named after all of them — unique, stable, and 81 characters of
        // hex that tell a person nothing. The readable-stem rule is the same one; it just has to
        // be applied by hand here, because the stem is not in the string.
        if let RepoSource::Archive(hash) = self {
            return format!("archive-{}-{:016x}", hash.short(), fnv1a(hash.as_bytes()));
        }
        // One mirror for every scratch run on a node: they all start from the same empty commit,
        // and each has its own branch and worktree off it, as runs on one repository do.
        if matches!(self, RepoSource::Scratch) {
            return "scratch".to_string();
        }
        let raw = self.clone_target();
        let name = raw
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .rsplit(['/', ':'])
            .next()
            .unwrap_or("repo");
        let sanitized: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let sanitized = sanitized.trim_matches('-');
        let stem = if sanitized.is_empty() {
            "repo"
        } else {
            sanitized
        };
        format!("{stem}-{:016x}", fnv1a(raw.as_bytes()))
    }
}

/// FNV-1a, written out rather than pulled in.
///
/// This value ends up in a directory name that persists across restarts and across
/// releases, so it has to hash the same forever. `DefaultHasher` explicitly does not
/// promise that.
#[must_use]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_shapes_git_accepts() {
        assert_eq!(
            RepoSource::parse("https://github.com/me/api.git"),
            RepoSource::Remote("https://github.com/me/api.git".into())
        );
        assert_eq!(
            RepoSource::parse("git@github.com:me/api.git"),
            RepoSource::Remote("git@github.com:me/api.git".into())
        );
        assert_eq!(
            RepoSource::parse("/home/owner/dev/api"),
            RepoSource::Local(PathBuf::from("/home/owner/dev/api"))
        );
        assert_eq!(
            RepoSource::parse("./relative/repo"),
            RepoSource::Local(PathBuf::from("./relative/repo"))
        );
    }

    /// ADR-0072: the spelling is the whole of what travels, so it has to round-trip, and it is
    /// fleetwide — any node can make an empty repository.
    #[test]
    fn a_scratch_workspace_round_trips_and_goes_anywhere() {
        assert_eq!(RepoSource::parse(SCRATCH), RepoSource::Scratch);
        assert_eq!(RepoSource::Scratch.clone_target(), SCRATCH);
        assert_eq!(RepoSource::Scratch.portability(), Portability::Fleetwide);
        assert_eq!(RepoSource::Scratch.archive(), None);
        // Exactly the spelling: a directory called `scratch:foo` is somebody's path.
        assert!(matches!(
            RepoSource::parse("scratch:foo"),
            RepoSource::Local(_)
        ));
    }

    #[test]
    fn an_archive_round_trips_through_the_string_a_run_spec_carries() {
        // `RunSpec::repo` is a `String`, so this spelling is the only thing that travels. If
        // `parse` and `clone_target` ever disagree, a run's workspace becomes unfindable on
        // every node including the one that submitted it.
        let hash = BlobHash::from_bytes([7; 32]);
        let spelled = format!("{ARCHIVE_PREFIX}{hash}");
        assert_eq!(RepoSource::parse(&spelled), RepoSource::Archive(hash));
        assert_eq!(RepoSource::parse(&spelled).clone_target(), spelled);
        assert_eq!(RepoSource::parse(&spelled).archive(), Some(hash));

        // The headline of ADR-0061: a workspace no node could clone can be placed anywhere.
        assert_eq!(
            RepoSource::parse(&spelled).portability(),
            Portability::Fleetwide
        );
    }

    #[test]
    fn a_malformed_archive_name_is_not_an_archive() {
        // The safe direction. A digest that will not parse must never become an archive some
        // peer is expected to serve; it falls through to `Local`, which is refused as a path
        // that is not here. Wrong-looking on purpose, and wrong the harmless way.
        for bad in [
            "archive:",
            "archive:not-hex",
            "archive:0011",
            "archive:zz1122334455667788990011223344556677889900112233445566778899001122",
        ] {
            assert!(
                matches!(RepoSource::parse(bad), RepoSource::Local(_)),
                "{bad} became an archive"
            );
            assert_eq!(RepoSource::parse(bad).archive(), None, "{bad}");
        }
    }

    #[test]
    fn an_archives_cache_key_is_readable_and_stable() {
        // It names a directory that outlives restarts and releases, so it is pinned here: the
        // generic path would have produced 81 characters of hex.
        let hash = BlobHash::from_bytes([7; 32]);
        let key = RepoSource::Archive(hash).cache_key();
        assert!(key.starts_with("archive-"), "{key}");
        assert_eq!(key, RepoSource::Archive(hash).cache_key(), "stable");
        assert_ne!(
            key,
            RepoSource::Archive(BlobHash::from_bytes([8; 32])).cache_key(),
            "two archives are two mirrors"
        );
    }

    #[test]
    fn local_paths_are_not_fleetwide() {
        // The fact the scheduler needs before placing, not after.
        assert_eq!(
            RepoSource::parse("/home/owner/dev/api").portability(),
            Portability::NodeLocal
        );
        assert_eq!(
            RepoSource::parse("https://github.com/me/api.git").portability(),
            Portability::Fleetwide
        );
    }
}
