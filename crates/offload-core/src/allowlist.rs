//! Per-tool allowlists: letting a run execute its own tests without granting it a shell.
//!
//! ADR-0008 shipped `AcceptEdits` as the default, which leaves a hole: running a test suite
//! is a `Bash` call, and headless gating denies it. So the natural loop — write code, run
//! tests, fix — needed `Full`, which grants unattended arbitrary execution. An allowlist
//! closes that gap at the right granularity.
//!
//! The mechanism is the agent's own `--allowedTools`, verified against `claude 2.1.220`:
//! `Bash(cargo test:*)` permits commands starting `cargo test` and denies everything else,
//! even under a deny-by-default permission mode.
//!
//! The subtlety worth the code in this module is that **an allowlist entry can quietly be
//! a general-execution grant**. `Bash(sh:*)` looks scoped and is not — `sh -c '…'` runs
//! anything. An allowlist that admits one of those provides the appearance of a limit
//! without the substance, which is worse than no limit at all, because it is trusted.

use serde::{Deserialize, Serialize};

/// Commands whose whole purpose is running an arbitrary string.
///
/// Not a security boundary on its own — it is a heuristic that catches the obvious
/// escapes. Deliberately excludes build tools (`cargo`, `make`, `npm`, `go`): those *do*
/// execute repository-controlled code, but that is inherent to building the repo at all,
/// and flagging them would make the feature useless for its actual purpose. Allowing a
/// build tool means trusting the repo's build scripts; that is a real caveat, and it is a
/// different one from handing over a shell.
///
/// Membership is decided by [`is_general_execution`] against the *program* a pattern names, not
/// against the text it was written as — see [`program_named`] for the four spellings that used
/// to walk straight past this list.
/// Two kinds of entry, and the grouping is what says whether something belongs.
///
/// **Interpreters** take a program as a string and run it. **Wrappers** take a *command* as
/// their arguments and run that, so the pattern constrains the wrapper and nothing after it —
/// `Bash(timeout:*)` permits `timeout 5 sh -c '…'` exactly as `Bash(env:*)` permits `env sh`.
/// Anything that fits either sentence belongs here; anything that does not, does not.
///
/// `npx`, `bunx`, `pipx` and `uvx` are interpreters by that test even though they look like
/// package managers: each fetches an arbitrary package from the network and runs it, which is
/// not the build-tool caveat below but the `curl | sh` one.
const GENERAL_EXECUTION: &[&str] = &[
    // Interpreters.
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "busybox",
    "eval",
    "exec",
    "python",
    "python2",
    "python3",
    "perl",
    "ruby",
    "node",
    "deno",
    "bun",
    "php",
    "lua",
    "awk",
    "gawk",
    "npx",
    "bunx",
    "pipx",
    "uvx",
    // Wrappers: the pattern names these and constrains nothing they go on to run.
    "env",
    "xargs",
    "find",
    "sudo",
    "doas",
    "su",
    "ssh",
    "nohup",
    "setsid",
    "script",
    "systemd-run",
    "timeout",
    "nice",
    "stdbuf",
    "flock",
    "watch",
    "time",
];

/// One entry in an allowlist, e.g. `Edit` or `Bash(cargo test:*)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolPattern(String);

/// How much a pattern actually grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Bounded to a specific command or tool.
    Scoped,
    /// Nominally scoped, effectively a shell.
    GeneralExecution,
    /// Openly unrestricted.
    Unrestricted,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("empty tool pattern")]
    Empty,
    #[error("unbalanced parentheses in `{0}`")]
    Unbalanced(String),
    #[error(
        "`{pattern}` grants {what}, which defeats the point of an allowlist. \
         Narrow it (for example `Bash(cargo test:*)`), or use --permission full if \
         unrestricted execution is genuinely what you want."
    )]
    TooBroad { pattern: String, what: String },
}

impl ToolPattern {
    pub fn parse(raw: &str) -> Result<Self, PatternError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(PatternError::Empty);
        }
        let opens = trimmed.matches('(').count();
        let closes = trimmed.matches(')').count();
        if opens != closes || opens > 1 || (opens == 1 && !trimmed.ends_with(')')) {
            return Err(PatternError::Unbalanced(trimmed.to_string()));
        }
        Ok(ToolPattern(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The tool name, e.g. `Bash`.
    #[must_use]
    pub fn tool(&self) -> &str {
        self.0.split_once('(').map_or(self.0.as_str(), |(t, _)| t)
    }

    /// The bracketed argument, e.g. `cargo test:*`.
    #[must_use]
    pub fn argument(&self) -> Option<&str> {
        let (_, rest) = self.0.split_once('(')?;
        rest.strip_suffix(')')
    }

    /// The command a `Bash` pattern actually permits, e.g. `cargo` for `cargo test:*`.
    #[must_use]
    pub fn command_word(&self) -> Option<&str> {
        let arg = self.argument()?.trim();
        let end = arg.find([':', ' ']).unwrap_or(arg.len());
        Some(arg[..end].trim())
    }

    #[must_use]
    pub fn risk(&self) -> Risk {
        // Non-Bash tools are bounded by what the tool does, and the worktree bounds the
        // file-touching ones.
        if !self.tool().eq_ignore_ascii_case("Bash") {
            return Risk::Scoped;
        }
        match self.argument().map(str::trim) {
            None => Risk::Unrestricted,
            Some(arg) if arg.is_empty() || arg == "*" || arg == ":*" => Risk::Unrestricted,
            Some(arg) => match program_named(arg) {
                // Nothing but environment assignments: `Bash(FOO=1:*)` constrains the
                // assignment and leaves the command after it entirely open.
                None => Risk::Unrestricted,
                Some(word) if word.is_empty() || word == "*" => Risk::Unrestricted,
                Some(word) if is_general_execution(&word) => Risk::GeneralExecution,
                Some(_) => Risk::Scoped,
            },
        }
    }
}

/// The program a `Bash` pattern's argument actually names, normalised for comparison.
///
/// `command_word` returns what was written; this returns what will *run*, and the gap between
/// them is where the whole check leaked. Four spellings of one program, none of them how
/// [`GENERAL_EXECUTION`] is written:
///
/// * **a path** — `/bin/sh`, `/usr/bin/env`, `./sh`, `../bin/bash`
/// * **a quoted word** — `"sh"`
/// * **an environment-assignment prefix** — `FOO=1 sh`
/// * **a version suffix** — `python3.11`, which is how python is spelled on most systems
///
/// Measured rather than reasoned about, on `claude 2.1.238`: a `.offload.toml` carrying
/// `Bash(/bin/sh:*)` passed [`ToolAllowlist::require_scoped`] — the check whose entire job is
/// stopping repository content from granting itself a shell — and the run then executed
/// `/bin/sh -c "cargo build --version"`, a command the same run had been denied on its own.
///
/// The same measurement settled why this is the *only* leak of its kind. The agent decomposes a
/// compound command and checks each part: `cargo test --version && cargo build --version` under
/// `Bash(cargo test:*)` is refused, naming the uncovered half. So a prefix grant does not admit
/// what comes after `&&`, and the one way a scoped-looking pattern becomes general execution is
/// for the program it names to be an interpreter — which is what this decides.
///
/// Returns `None` when the argument names no program at all.
fn program_named(argument: &str) -> Option<String> {
    // The agent's own syntax is `<command prefix>:<glob>`, so the prefix is what will run.
    let prefix = argument.split(':').next().unwrap_or(argument);
    let word = prefix
        .split_whitespace()
        // `VAR=value` before the program is shell syntax, not the program.
        .find(|token| !token.contains('='))?
        .trim_matches(['"', '\''])
        .trim();
    // A path names its last component; everything before it is where it lives. Quotes are
    // trimmed on both sides of that split because a shell accepts them on either — `"/bin/sh"`
    // and `/bin/"sh"` both run `/bin/sh`, and it was the second that a generated case found.
    let base = word.rsplit('/').next().unwrap_or(word);
    Some(base.trim_matches(['"', '\'']).to_ascii_lowercase())
}

/// Does this program name an interpreter, however it is versioned?
///
/// The trailing-version strip is why this is a function: `python3.11` and `python3.12` are how
/// the program is spelled on a real machine, and listing every release is a list that goes stale
/// silently. It can over-match — a program whose name is a listed one plus digits — and that is
/// the direction to be wrong in: a false positive is a refusal that names itself, and a false
/// negative is a shell the operator was told was scoped.
fn is_general_execution(program: &str) -> bool {
    let stem = program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    GENERAL_EXECUTION
        .iter()
        .any(|c| c.eq_ignore_ascii_case(program) || c.eq_ignore_ascii_case(stem))
}

impl std::fmt::Display for ToolPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Does a pattern's tool name select this call's tool?
///
/// Equality, and one prefix rule for MCP: `mcp__mail` is the agent's own spelling for every tool
/// the `mail` server offers, so it selects `mcp__mail__read_inbox`. Anchored on the double
/// underscore rather than on a bare prefix, or `mcp__mail` would also select a different server
/// called `mcp__mailing-list`.
fn names_the_same_tool(pattern: &str, tool: &str) -> bool {
    if pattern.eq_ignore_ascii_case(tool) {
        return true;
    }
    pattern.starts_with("mcp__")
        && tool.len() > pattern.len()
        && tool[..pattern.len()].eq_ignore_ascii_case(pattern)
        && tool[pattern.len()..].starts_with("__")
}

/// A set of tool patterns, in the order they will be given to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolAllowlist {
    patterns: Vec<ToolPattern>,
}

impl ToolAllowlist {
    pub fn parse<I, S>(items: I) -> Result<Self, PatternError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut patterns = Vec::new();
        for item in items {
            // A single entry may be comma-separated, since that is how people write lists.
            for part in item.as_ref().split(',') {
                if part.trim().is_empty() {
                    continue;
                }
                let pattern = ToolPattern::parse(part)?;
                if !patterns.contains(&pattern) {
                    patterns.push(pattern);
                }
            }
        }
        Ok(ToolAllowlist { patterns })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    #[must_use]
    pub fn patterns(&self) -> &[ToolPattern] {
        &self.patterns
    }

    /// Does a grant in this list already cover a call the agent is about to make?
    ///
    /// Used for one thing only (ADR-0017): deciding whether a permission question is worth
    /// asking a person. **Its failure direction is what makes it safe to have at all.** A match
    /// means "do not ask, let the agent apply its own rules to this call", never "allow it" — so
    /// a matcher that is *wider* than the agent's costs a question that could have been asked,
    /// and can never grant something the agent would have gated. That is the opposite of the
    /// usual risk with a second implementation of a pattern language, and it is the reason this
    /// exists rather than the agent's matcher being reimplemented for real.
    ///
    /// Three shapes, which is the whole of the syntax `ToolPattern` allows:
    /// `Bash` covers every call to the tool, `Bash(cargo test:*)` covers arguments starting with
    /// `cargo test`, and `Bash(cargo test)` covers exactly that argument.
    ///
    /// Plus one the agent spells rather than we do: `mcp__<server>` names *every* tool a server
    /// offers, and the tools themselves are `mcp__<server>__<tool>` (ADR-0011). Granting a run a
    /// resource emits the first, so a matcher that compared whole names would have disagreed
    /// with the agent about the one pattern this project generates for itself — the drift a
    /// second implementation of somebody's pattern language exists to be worried about, arriving
    /// through our own front door.
    #[must_use]
    pub fn covers(&self, tool: &str, argument: &str) -> bool {
        self.patterns.iter().any(|pattern| {
            if !names_the_same_tool(pattern.tool(), tool) {
                return false;
            }
            match pattern.argument() {
                None => true,
                Some(wanted) => match wanted.trim().strip_suffix('*') {
                    Some(prefix) => argument
                        .trim()
                        .starts_with(prefix.trim_end_matches(':').trim_end()),
                    None => argument.trim() == wanted.trim(),
                },
            }
        })
    }

    /// Union two allowlists, preserving order and dropping duplicates.
    ///
    /// Union rather than override: these are grants layered from node config, repo config,
    /// and the command line, and a more specific layer silently *removing* a grant the
    /// operator configured would be surprising in the wrong direction.
    #[must_use]
    pub fn merged_with(&self, other: &ToolAllowlist) -> ToolAllowlist {
        let mut patterns = self.patterns.clone();
        for pattern in &other.patterns {
            if !patterns.contains(pattern) {
                patterns.push(pattern.clone());
            }
        }
        ToolAllowlist { patterns }
    }

    /// Patterns that grant more than they appear to.
    #[must_use]
    pub fn risky(&self) -> Vec<(&ToolPattern, Risk)> {
        self.patterns
            .iter()
            .map(|p| (p, p.risk()))
            .filter(|(_, risk)| *risk != Risk::Scoped)
            .collect()
    }

    /// Reject anything that is not genuinely scoped.
    ///
    /// Applied to allowlists that come from the repository rather than from the operator:
    /// a checked-in `.offload.toml` is content, and content should not be able to grant
    /// itself a shell.
    pub fn require_scoped(&self) -> Result<(), PatternError> {
        let risky = self.risky();
        let Some((pattern, risk)) = risky.first() else {
            return Ok(());
        };
        Err(PatternError::TooBroad {
            pattern: pattern.to_string(),
            what: match risk {
                Risk::Unrestricted => "unrestricted shell access".to_string(),
                Risk::GeneralExecution => format!(
                    "general execution via `{}`",
                    pattern.command_word().unwrap_or("?")
                ),
                // `risky()` only yields non-Scoped entries, but expressing that as a
                // panic would trade a config error for a crashed daemon.
                Risk::Scoped => "more than it appears to".to_string(),
            },
        })
    }

    /// Arguments to append after `--allowedTools`.
    ///
    /// One argv entry per pattern: patterns contain spaces (`Bash(cargo test:*)`), so a
    /// space-separated single argument would be ambiguous. Verified against the real CLI.
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        self.patterns.iter().map(|p| p.0.clone()).collect()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_grant_that_already_covers_a_call_means_there_is_nothing_to_ask_about() {
        let list =
            ToolAllowlist::parse(["Bash(cargo test:*)", "Read", "WebFetch(https://docs.rs)"])
                .expect("parse");

        // The prefix form, which is what nearly every grant is.
        assert!(list.covers("Bash", "cargo test --all"));
        assert!(list.covers("Bash", "cargo test"));
        assert!(!list.covers("Bash", "cargo build"));
        // A bare tool covers everything it does.
        assert!(list.covers("Read", "/anything/at/all"));
        // And an exact grant is exact.
        assert!(list.covers("WebFetch", "https://docs.rs"));
        assert!(!list.covers("WebFetch", "https://docs.rs/serde"));
        // A tool nobody granted anything for is always worth asking about.
        assert!(!list.covers("Write", "/etc/passwd"));
        // Tool names are the agent's, and case is not where a security decision should live.
        assert!(list.covers("bash", "cargo test -q"));
    }

    #[test]
    fn granting_a_whole_mcp_server_covers_its_tools() {
        // The one pattern this project generates for itself (ADR-0011): granting a run a
        // resource emits `mcp__<server>`, which is the agent's spelling for every tool that
        // server offers. A matcher comparing whole names would disagree with the agent about it.
        let list = ToolAllowlist::parse(["mcp__mail"]).expect("parse");
        assert!(list.covers("mcp__mail__read_inbox", ""));
        assert!(list.covers("mcp__mail", ""));
        // Anchored on the separator, or one server's grant would reach another's tools.
        assert!(!list.covers("mcp__mailing-list__post", ""));
        assert!(!list.covers("mcp__cal__add_event", ""));
        // And it is a rule about MCP, not a prefix rule about everything.
        let bash = ToolAllowlist::parse(["Bash"]).expect("parse");
        assert!(!bash.covers("BashOutput", "x"));
    }
    use super::*;

    fn list(items: &[&str]) -> ToolAllowlist {
        ToolAllowlist::parse(items).expect("valid patterns")
    }

    #[test]
    fn parses_the_shapes_the_agent_accepts() {
        let l = list(&["Edit", "Bash(cargo test:*)"]);
        assert_eq!(l.patterns()[0].tool(), "Edit");
        assert_eq!(l.patterns()[0].argument(), None);
        assert_eq!(l.patterns()[1].tool(), "Bash");
        assert_eq!(l.patterns()[1].argument(), Some("cargo test:*"));
        assert_eq!(l.patterns()[1].command_word(), Some("cargo"));
    }

    #[test]
    fn a_scoped_build_command_is_scoped() {
        // The case the whole feature exists for.
        for pattern in [
            "Bash(cargo test:*)",
            "Bash(npm test:*)",
            "Bash(make check:*)",
            "Bash(pytest:*)",
            "Edit",
            "Write",
        ] {
            assert_eq!(
                list(&[pattern]).patterns()[0].risk(),
                Risk::Scoped,
                "{pattern} should be scoped"
            );
        }
    }

    #[test]
    fn a_shell_disguised_as_a_scoped_grant_is_caught() {
        // The failure this module exists to prevent: `Bash(sh:*)` looks like a narrow
        // grant and is a general shell. An allowlist containing one provides the
        // appearance of a limit without the substance.
        for pattern in [
            "Bash(sh:*)",
            "Bash(bash -c:*)",
            "Bash(python:*)",
            "Bash(xargs:*)",
            "Bash(sudo:*)",
            "Bash(env:*)",
            "Bash(find:*)",
            "Bash(ssh:*)",
        ] {
            assert_eq!(
                list(&[pattern]).patterns()[0].risk(),
                Risk::GeneralExecution,
                "{pattern} should be flagged"
            );
        }
    }

    /// Found by a property test, then measured against the real agent.
    ///
    /// Every one of these used to read as `Scoped`, which is what `require_scoped` accepts from
    /// a repository's own `.offload.toml` — the check whose entire job is stopping content from
    /// granting itself a shell. On `claude 2.1.238`, a run allowed `Bash(/bin/sh:*)` and nothing
    /// else executed `/bin/sh -c "cargo build --version"`; the same run, asked to run
    /// `cargo build --version` directly, was refused. So the pattern was not merely mislabelled,
    /// it was a working escape from the label.
    ///
    /// The same measurement is why this is the only leak of its shape: the agent decomposes a
    /// compound command and checks the parts, refusing
    /// `cargo test --version && cargo build --version` under `Bash(cargo test:*)` and naming the
    /// uncovered half. A prefix grant does not admit what comes after `&&`, so the one way a
    /// scoped-looking pattern turns into general execution is for the program it names to be an
    /// interpreter — which is exactly what `risk` decides, and now decides about the program
    /// rather than about the text.
    #[test]
    fn a_shell_reached_by_a_path_or_a_version_number_is_still_a_shell() {
        for pattern in [
            // A path, absolute or relative.
            "Bash(/bin/sh:*)",
            "Bash(/usr/bin/env:*)",
            "Bash(./sh:*)",
            "Bash(../bin/bash:*)",
            // How python is actually spelled on a real machine.
            "Bash(python3.11:*)",
            "Bash(python3.12 -c:*)",
            // Quoted, either side of the path separator.
            "Bash(\"sh\":*)",
            "Bash(\"/bin/sh\":*)",
            "Bash(/bin/\"sh\":*)",
            // An environment assignment is shell syntax, not the program.
            "Bash(FOO=1 sh:*)",
            "Bash(FOO=1 BAR=2 /bin/bash -lc:*)",
        ] {
            assert_eq!(
                list(&[pattern]).patterns()[0].risk(),
                Risk::GeneralExecution,
                "{pattern} should be flagged"
            );
            list(&[pattern])
                .require_scoped()
                .expect_err("a repo must not be able to write this");
        }

        // A pattern that is nothing but an assignment constrains the assignment and leaves
        // everything after it open, so it is unrestricted rather than merely a named shell.
        assert_eq!(
            list(&["Bash(FOO=1:*)"]).patterns()[0].risk(),
            Risk::Unrestricted
        );

        // And the other direction, which is what keeps the feature usable: a build tool named
        // by path is still a build tool, and trusting the repo's build scripts is the documented
        // caveat rather than a shell.
        for pattern in [
            "Bash(cargo test:*)",
            "Bash(/usr/local/bin/cargo test:*)",
            "Bash(./scripts/test.sh:*)",
            "Bash(make check:*)",
        ] {
            assert_eq!(
                list(&[pattern]).patterns()[0].risk(),
                Risk::Scoped,
                "{pattern} should stay usable"
            );
        }
    }

    #[test]
    fn openly_unrestricted_patterns_are_recognised() {
        for pattern in ["Bash", "Bash(*)", "Bash(:*)", "Bash()"] {
            assert_eq!(
                list(&[pattern]).patterns()[0].risk(),
                Risk::Unrestricted,
                "{pattern} should be unrestricted"
            );
        }
    }

    #[test]
    fn repo_supplied_allowlists_cannot_grant_a_shell() {
        // A checked-in .offload.toml is content, and content must not be able to grant
        // itself unrestricted execution just because someone cloned the repo.
        list(&["Bash(cargo test:*)"])
            .require_scoped()
            .expect("scoped patterns are fine");

        let err = list(&["Bash(cargo test:*)", "Bash(sh:*)"])
            .require_scoped()
            .expect_err("must reject the shell");
        let message = err.to_string();
        assert!(message.contains("sh"), "{message}");
        assert!(
            message.contains("--permission full"),
            "must name the honest alternative: {message}"
        );
    }

    #[test]
    fn layers_union_and_deduplicate() {
        // Node config, repo config, and --allow are all grants; a later layer removing an
        // earlier grant would surprise in the dangerous direction.
        let node = list(&["Bash(git status:*)"]);
        let repo = list(&["Bash(cargo test:*)", "Bash(git status:*)"]);
        let cli = list(&["Edit"]);

        let merged = node.merged_with(&repo).merged_with(&cli);
        assert_eq!(
            merged.to_args(),
            vec!["Bash(git status:*)", "Bash(cargo test:*)", "Edit"],
            "order preserved, duplicate dropped"
        );
    }

    #[test]
    fn comma_separated_entries_are_split() {
        let l = list(&["Edit,Bash(cargo test:*)"]);
        assert_eq!(l.patterns().len(), 2);
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        assert_eq!(ToolPattern::parse(""), Err(PatternError::Empty));
        assert!(matches!(
            ToolPattern::parse("Bash(cargo test"),
            Err(PatternError::Unbalanced(_))
        ));
        assert!(matches!(
            ToolPattern::parse("Bash(a)(b)"),
            Err(PatternError::Unbalanced(_))
        ));
    }

    #[test]
    fn args_are_one_entry_per_pattern() {
        // Patterns contain spaces, so a single space-joined argument would be ambiguous.
        // Verified against the real CLI.
        let args = list(&["Bash(cargo test:*)", "Edit"]).to_args();
        assert_eq!(args, vec!["Bash(cargo test:*)".to_string(), "Edit".into()]);
    }

    #[test]
    fn an_empty_allowlist_produces_no_arguments() {
        // Passing a bare `--allowedTools` with nothing after it would change the agent's
        // behaviour rather than leave it alone.
        assert!(ToolAllowlist::default().is_empty());
        assert!(ToolAllowlist::default().to_args().is_empty());
    }
}
