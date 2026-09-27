//! Looking inside a run's workspace, read-only (ADR-0075).
//!
//! What a person asks for from a phone: "what did it write". The checkout if it is still here,
//! which has the uncommitted work, else the run's branch in the mirror, which has what was
//! committed. Paths are relative to the workspace root and are held to it: `..` and absolute
//! paths are refused, and a symlink that resolves outside the checkout is refused too. `.git` is
//! not shown. Files are capped, and one that is not text is shown as its size.

use crate::{git, WorkspaceManager};
use offload_core::RepoSource;
use offload_core::RunId;
use std::path::{Component, Path, PathBuf};

/// The most of a file shown. A phone screen reads far less, and a frame must hold it.
pub const TEXT_CAP: usize = 256 * 1024;
/// The most entries listed in one directory.
pub const LIST_CAP: usize = 500;
/// How much of a file is looked at to tell text from not: git's own rule, a NUL in the first 8 KiB.
const SNIFF: usize = 8 * 1024;

/// What was shown, and from where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peek {
    /// `true` for the checkout, `false` for the run's branch.
    pub from_checkout: bool,
    pub shown: Shown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    /// Entries as (name, is a directory, size in bytes), directories first; and whether cut.
    Listing(Vec<(String, bool, u64)>, bool),
    /// Text, its whole size, and whether cut.
    Text(String, u64, bool),
    /// Not text, and its size.
    Binary(u64),
}

/// `rel` as components under the root, or why not. `""` and `"."` are the root.
pub fn relative(rel: &str) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(name) => {
                if name == ".git" {
                    return Err("`.git` is not shown".into());
                }
                out.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "`{rel}` is outside the workspace — paths are relative to it"
                ));
            }
        }
    }
    Ok(out)
}

fn text_or_binary(bytes: &[u8], whole: u64) -> Shown {
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        return Shown::Binary(whole);
    }
    let cut = bytes.len() > TEXT_CAP;
    let slice = &bytes[..bytes.len().min(TEXT_CAP)];
    Shown::Text(String::from_utf8_lossy(slice).into_owned(), whole, cut)
}

impl WorkspaceManager {
    /// Show `rel` in `run`'s workspace: its checkout here if there is one, else `branch` in
    /// `source`'s mirror here. `Err` in words: neither is here, the path is outside, or missing.
    pub async fn peek(
        &self,
        run: RunId,
        source: &RepoSource,
        branch: &str,
        rel: &str,
    ) -> Result<Peek, String> {
        let rel = relative(rel)?;
        let checkout = self.worktree_path(run);
        if checkout.is_dir() {
            return peek_checkout(&checkout, &rel).map(|shown| Peek {
                from_checkout: true,
                shown,
            });
        }
        let mirror = self.mirror_path(source);
        if !self.has_branch(source, branch).await {
            return Err(format!(
                "run {} has no checkout on this machine and its branch is not in the mirror here",
                run.short()
            ));
        }
        peek_branch(&mirror, branch, &rel).await.map(|shown| Peek {
            from_checkout: false,
            shown,
        })
    }
}

fn peek_checkout(root: &Path, rel: &Path) -> Result<Shown, String> {
    let io = |e: std::io::Error| e.to_string();
    let base = root.canonicalize().map_err(io)?;
    let target = base.join(rel);
    let resolved = target
        .canonicalize()
        .map_err(|_| format!("nothing at `{}`", rel.display()))?;
    // A symlink the agent made may point anywhere; what it resolves to must be inside.
    if !resolved.starts_with(&base) {
        return Err(format!("`{}` leads outside the workspace", rel.display()));
    }
    let meta = std::fs::metadata(&resolved).map_err(io)?;
    if meta.is_dir() {
        let mut entries: Vec<(String, bool, u64)> = std::fs::read_dir(&resolved)
            .map_err(io)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != ".git")
            .map(|e| {
                let meta = e.metadata().ok();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
                    meta.map_or(0, |m| m.len()),
                )
            })
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let cut = entries.len() > LIST_CAP;
        entries.truncate(LIST_CAP);
        return Ok(Shown::Listing(entries, cut));
    }
    let whole = meta.len();
    let mut bytes = Vec::new();
    use std::io::Read;
    std::fs::File::open(&resolved)
        .map_err(io)?
        .take(u64::try_from(TEXT_CAP + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(io)?;
    Ok(text_or_binary(&bytes, whole))
}

async fn peek_branch(mirror: &Path, branch: &str, rel: &Path) -> Result<Shown, String> {
    let spec = format!("{branch}:{}", rel.display());
    let kind = git(Some(mirror), &["cat-file", "-t", &spec])
        .await
        .map_err(|_| format!("nothing at `{}` on the run's branch", rel.display()))?;
    match kind.trim() {
        "tree" => {
            let listing = git(Some(mirror), &["ls-tree", "-l", &spec])
                .await
                .map_err(|e| e.to_string())?;
            let mut entries: Vec<(String, bool, u64)> = listing
                .lines()
                .filter_map(|line| {
                    let (meta, name) = line.split_once('\t')?;
                    let mut fields = meta.split_whitespace();
                    let _mode = fields.next()?;
                    let kind = fields.next()?;
                    let _hash = fields.next()?;
                    let size = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    Some((name.to_string(), kind == "tree", size))
                })
                .collect();
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let cut = entries.len() > LIST_CAP;
            entries.truncate(LIST_CAP);
            Ok(Shown::Listing(entries, cut))
        }
        "blob" => {
            let size: u64 = git(Some(mirror), &["cat-file", "-s", &spec])
                .await
                .map_err(|e| e.to_string())?
                .trim()
                .parse()
                .unwrap_or(0);
            let bytes = crate::git_bytes(Some(mirror), &["cat-file", "blob", &spec])
                .await
                .map_err(|e| e.to_string())?;
            Ok(text_or_binary(&bytes, size))
        }
        other => Err(format!(
            "`{}` is a {other}, which is not shown",
            rel.display()
        )),
    }
}
