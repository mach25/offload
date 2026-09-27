//! `.offload.toml` — a repository declaring what its runs need.
//!
//! Only the repo knows that its tests are `cargo test` and not `npm test`, so this is the
//! layer where an allowlist belongs. Without it, every submitter has to retype the same
//! `--allow` flags, and in practice will reach for `--permission full` instead, which is
//! the outcome ADR-0008 was trying to avoid.
//!
//! **This file is content, not configuration the operator wrote.** Anyone who can open a
//! pull request can edit it, and it arrives with any repo that gets cloned. So what it may
//! grant is capped: [`ToolAllowlist::require_scoped`] rejects anything that amounts to a
//! shell, and the operator can switch the whole mechanism off. A repo may say "run my
//! tests"; it may not say "give me `sh`".

use offload_core::ToolAllowlist;
use std::path::Path;

/// The filename read from a run's worktree root.
pub const REPO_CONFIG_FILE: &str = ".offload.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RepoConfig {
    pub run: RunSection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunSection {
    /// Tools this repo's runs may use without prompting, e.g. `["Bash(cargo test:*)"]`.
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepoConfigError {
    #[error("{REPO_CONFIG_FILE} is not valid TOML: {0}")]
    Parse(String),
    #[error("{REPO_CONFIG_FILE} declares a tool grant it is not allowed to: {0}")]
    Rejected(#[from] offload_core::PatternError),
}

/// Read and validate the allowlist a worktree declares for itself.
///
/// Returns an empty allowlist when there is no file — the overwhelmingly common case, and
/// not something to report as a problem.
pub fn load_allowlist(worktree: &Path) -> Result<ToolAllowlist, RepoConfigError> {
    let path = worktree.join(REPO_CONFIG_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(ToolAllowlist::default());
    };

    let config: RepoConfig =
        toml::from_str(&text).map_err(|e| RepoConfigError::Parse(e.to_string()))?;
    let allow = ToolAllowlist::parse(&config.run.allow)?;

    // The cap. A repo describing its own test command is reasonable; a repo granting
    // itself general execution is the thing an allowlist exists to prevent.
    allow.require_scoped()?;

    if !allow.is_empty() {
        tracing::info!(
            patterns = allow.patterns().len(),
            "repo declares a tool allowlist"
        );
    }
    Ok(allow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("offload-repocfg-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn no_file_is_not_an_error() {
        // The common case. Reporting it would train people to ignore the output.
        let dir = scratch("none");
        assert!(load_allowlist(&dir).expect("ok").is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repo_can_declare_its_own_test_command() {
        // The case the feature exists for: this is what lets AcceptEdits runs be useful.
        let dir = scratch("ok");
        std::fs::write(
            dir.join(REPO_CONFIG_FILE),
            "[run]\nallow = [\"Bash(cargo test:*)\", \"Bash(cargo build:*)\"]\n",
        )
        .expect("write");

        let allow = load_allowlist(&dir).expect("valid");
        assert_eq!(
            allow.to_args(),
            vec![
                "Bash(cargo test:*)".to_string(),
                "Bash(cargo build:*)".into()
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_repo_cannot_grant_itself_a_shell() {
        // The security property. `.offload.toml` arrives with any cloned repo and can be
        // edited by anyone who can open a pull request, so it must not be able to convert
        // an AcceptEdits run into unrestricted execution.
        let dir = scratch("shell");
        std::fs::write(
            dir.join(REPO_CONFIG_FILE),
            "[run]\nallow = [\"Bash(cargo test:*)\", \"Bash(sh:*)\"]\n",
        )
        .expect("write");

        let err = load_allowlist(&dir).expect_err("must reject");
        assert!(matches!(err, RepoConfigError::Rejected(_)), "{err}");
        assert!(err.to_string().contains("sh"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_openly_unrestricted_grant_is_rejected_too() {
        let dir = scratch("star");
        std::fs::write(dir.join(REPO_CONFIG_FILE), "[run]\nallow = [\"Bash(*)\"]\n")
            .expect("write");
        assert!(load_allowlist(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_file_is_reported_rather_than_ignored() {
        // Silently ignoring it would leave the run denied with no explanation of why the
        // allowlist the repo thought it had did not apply.
        let dir = scratch("bad");
        std::fs::write(
            dir.join(REPO_CONFIG_FILE),
            "[run]\nallow = \"not a list\"\n",
        )
        .expect("write");
        assert!(matches!(
            load_allowlist(&dir),
            Err(RepoConfigError::Parse(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        let dir = scratch("typo");
        std::fs::write(dir.join(REPO_CONFIG_FILE), "[run]\nallowed = [\"Edit\"]\n").expect("write");
        assert!(load_allowlist(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
