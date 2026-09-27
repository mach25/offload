//! Which agents this device has, and whether they can actually be used.
//!
//! "Installed" is easy. "Authenticated" is the one that matters and the one we can only
//! infer — so it is inferred conservatively. A node that claims auth it does not have will
//! win bids and then fail every run it takes, which is a much worse failure than a node
//! that under-reports and sits idle.

use offload_core::capability::{AccountId, AgentDetails, AgentKind, Capability};
use std::path::PathBuf;

/// Conservative default. Concurrent agent runs contend for the same rate limit far more
/// than for CPU, so this is about the account, not the machine.
const DEFAULT_MAX_CONCURRENT: u32 = 2;

/// What a machine with an ordinary install runs, and the default `AgentConfig::binary` names.
///
/// A bare name, resolved on `PATH` — which is a guess about the machine, and the reason
/// everything here takes the binary as an argument rather than reaching for this.
pub const DEFAULT_CLAUDE_BINARY: &str = "claude";

/// The agents on this device, asking about the program named.
///
/// **The binary is an argument because the caller is the one that will spawn it.** This used to
/// ask `claude` on `PATH` unconditionally while `offloadd` spawns `agent.binary`, so the setting
/// whose entire job is "where the agent is" decided what ran and not what this device *claimed* —
/// and the two disagree exactly when somebody uses it. Both directions were live: an owner whose
/// agent is at a path (a version pin, a wrapper, a phone) advertised the version and the
/// authentication of a different program, and one with no `claude` on `PATH` at all advertised no
/// agent and never bid, on a machine whose configured agent works perfectly.
#[must_use]
pub fn detect_agents(claude: &std::path::Path, state: Option<&std::path::Path>) -> Vec<Capability> {
    probe_agent(&AgentKind::ClaudeCode, claude, state)
        .into_iter()
        .collect()
}

#[must_use]
pub fn probe_agent(
    kind: &AgentKind,
    binary: &std::path::Path,
    state: Option<&std::path::Path>,
) -> Option<Capability> {
    match kind {
        AgentKind::ClaudeCode => probe_claude_code(binary, state),
        AgentKind::Other(_) => None,
    }
}

/// Installed and what version comes from the **binary**; authenticated comes from the *user's*
/// credential directory, because that is what each one is actually a fact about — two installs on
/// one login share the credential, and `$CLAUDE_CONFIG_DIR` moves it for both.
fn probe_claude_code(
    binary: &std::path::Path,
    state: Option<&std::path::Path>,
) -> Option<Capability> {
    let version = crate::probe_version(binary, &["--version"])?;
    let (authenticated, account) = claude_auth(state);

    let details = AgentDetails {
        version: version.clone(),
        // Not probed here: the list is the agent's own answer to `initialize`, which takes a
        // couple of seconds and changes only with the agent's version, so the node reads it on
        // its own cadence and folds it in (ADR-0080). This was a constant, "declared, not
        // verified", and it had gone stale — `claude-fable-5` against a current Fable 5.1.
        models: Vec::new(),
        max_concurrent: DEFAULT_MAX_CONCURRENT,
    };

    Some(
        Capability::agent(AgentKind::ClaudeCode, details, authenticated)
            .with_identity(account)
            .described(format!(
                "Claude Code {version}{}",
                if authenticated {
                    ""
                } else {
                    " (not authenticated)"
                }
            )),
    )
}

/// Best-effort auth check.
///
/// Returns an opaque [`AccountId`] fingerprint where one can be derived — not a credential,
/// and never sent anywhere as one. It exists so the scheduler can notice that several nodes
/// share an account and therefore share a rate limit.
fn claude_auth(state: Option<&std::path::Path>) -> (bool, Option<AccountId>) {
    // An explicit API key in the environment beats anything on disk. Fingerprinted by its
    // *value*: two nodes holding two different keys are two accounts sharing no rate limit,
    // and the label this replaced was the name of the variable, so every such node reported
    // the same account and would have shared a cap it does not share. A key that is not UTF-8
    // is authenticated with no account rather than a guess — the probe's usual rule.
    if let Some(key) = std::env::var_os("ANTHROPIC_API_KEY").filter(|k| !k.is_empty()) {
        return (true, key.to_str().map(AccountId::of_api_key));
    }

    let Some(home) = home_dir() else {
        return (false, None);
    };
    let dir = claude_config_dir(&home, state);

    // Credentials file present means an interactive login has happened. We deliberately do
    // not read or parse it: knowing it exists is enough, and opening it is not our business.
    // Where the owner nominated the directory, nothing outside it counts — see
    // `account_fingerprint`. `None` is that instruction rather than a missing home.
    let outside = state.map_or(Some(home.as_path()), |_| None);
    let credentials = dir.join(".credentials.json");
    if credentials.exists() {
        return (true, account_fingerprint(&dir, outside));
    }

    // macOS keeps the token in the keychain instead, so fall back to config presence. Both
    // places, because where `.claude.json` lands when the config directory has been moved is
    // not something this has been able to verify — and checking two paths costs a `stat` while
    // guessing the wrong one reports a logged-in Mac as unauthenticated.
    if std::env::consts::OS == "macos"
        && (dir.join(".claude.json").exists()
            || outside.is_some_and(|home| home.join(".claude.json").exists()))
    {
        return (true, account_fingerprint(&dir, outside));
    }

    (false, None)
}

/// Where Claude Code keeps its state for this user: what the owner nominated, else
/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
///
/// **The nominated path wins, and it is what selects the account** (ADR-0028). Before it existed
/// this was a fact about whatever shell started the daemon: `offloadd` launched from a terminal
/// with `$CLAUDE_CONFIG_DIR` set ran on one login and from a service manager on another, with
/// nothing in `node.toml` able to say which — the same shape as the probe asking `claude` on
/// `PATH` while the daemon spawns `agent.binary`, one field over.
///
/// **A second copy of the rule in `offload_agent::transcript::config_dir`**, and deliberately so:
/// a probe that pulled in the whole agent adapter — tokio and all — to resolve one path would be
/// the dependency doing more harm than the duplication. Both are three lines and both are tested.
/// They must not disagree: this one decides whether the node claims it can run agents at all, and
/// the other decides whether its checkpoints work, so a fleet where they differ has a node that
/// wins bids and then captures nothing.
fn claude_config_dir(home: &std::path::Path, nominated: Option<&std::path::Path>) -> PathBuf {
    match nominated {
        Some(dir) => dir.to_path_buf(),
        None => resolve_config_dir(home, std::env::var_os("CLAUDE_CONFIG_DIR")),
    }
}

/// The pure half of [`claude_config_dir`], so the rule can be tested without touching the
/// environment of whatever else is running in this process — the seam
/// `offload_agent::transcript::resolve_config_dir` has, and which this copy lacked. Its test
/// branched on the ambient variable to decide what to assert, which is a test that passes
/// either way rather than one that pins the rule.
fn resolve_config_dir(home: &std::path::Path, configured: Option<std::ffi::OsString>) -> PathBuf {
    match configured {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        // An empty variable is somebody unsetting it awkwardly, not a request to use the
        // current directory.
        _ => home.join(".claude"),
    }
}

/// A stable, opaque label for "the account this machine is logged in as".
///
/// Derived from the account uuid the agent itself records, so **two machines logged into the
/// same account produce the same fingerprint** — which is the whole point, and what the
/// previous version (`$USER` and `$HOME`) could not do. That one was honest about being a
/// placeholder, and its cost was that per-account rate-limit accounting compared a value that
/// matched nothing anywhere: `bid::evaluate`'s account pressure was live code that could
/// never fire.
///
/// `None` when there is no uuid to read, rather than something machine-shaped: an account this
/// cannot identify is one whose runs must not be counted against another node's cap.
fn account_fingerprint(dir: &std::path::Path, home: Option<&std::path::Path>) -> Option<AccountId> {
    // `.claude.json` is *inside* the config directory when that has been moved and at
    // `~/.claude.json` when it has not — verified on a machine with both. This is the same
    // two-path ambiguity the macOS branch above hedges against, and checking both costs a
    // `stat` while guessing wrong loses the account entirely.
    //
    // **Except where the owner nominated the directory** (ADR-0028), which is what `home: None`
    // means. There the fallback stops being a hedge and becomes a wrong answer: the whole point
    // of naming a directory is that this device runs on *that* login, so a directory with no
    // settings file in it is one nobody has logged into — and answering with `$HOME`'s account
    // reports, gossips and rate-limit-accounts the node as the login the owner did not choose.
    // Found by walking it: a daemon pointed at a fresh state directory reported the personal
    // account, from a file two directories up.
    [
        Some(dir.join(".claude.json")),
        home.map(|h| h.join(".claude.json")),
    ]
    .into_iter()
    .flatten()
    .find_map(|path| account_uuid_in(&path))
    .map(|uuid| AccountId::of_account(&uuid))
}

/// Read `oauthAccount.accountUuid` out of the agent's own settings file.
///
/// The uuid and not the email address beside it: the fingerprint is gossiped to every peer and
/// rendered into `offload explain`'s text, and a digest of a guessable string is reversible in
/// practice however opaque it looks. This file is settings and history rather than a credential
/// store — the tokens live in `.credentials.json`, which nothing here opens — but it is read
/// for exactly one field regardless, and a parse failure is `None` rather than a panic.
fn account_uuid_in(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let uuid = json.get("oauthAccount")?.get("accountUuid")?.as_str()?;
    let uuid = uuid.trim();
    (!uuid.is_empty()).then(|| uuid.to_string())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A scratch directory of our own, named after the test so two of them cannot collide.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("offload-probe-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn write_settings(dir: &std::path::Path, body: &str) {
        std::fs::write(dir.join(".claude.json"), body).expect("write settings");
    }

    fn settings_with(uuid: &str) -> String {
        format!(r#"{{"oauthAccount":{{"accountUuid":"{uuid}","emailAddress":"a@b.c"}}}}"#)
    }

    /// A program that answers `--version` and nothing else, at a path of our choosing.
    fn fake_agent(name: &str, version: &str) -> PathBuf {
        let dir = scratch(name);
        let path = dir.join("not-called-claude");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho '{version} (Claude Code)'\n"),
        )
        .expect("write agent");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    #[test]
    fn the_agent_probed_is_the_binary_it_was_handed() {
        // This asked `claude` on `PATH` unconditionally, while the daemon spawns `agent.binary` —
        // so the setting that decides what runs decided nothing about what this device claimed.
        // Deliberately *not* named `claude`: were the argument ignored, this reports whatever
        // this machine happens to have on `PATH`, which is a version that is not 9.9.9 or no
        // agent at all. Either way it goes red.
        let binary = fake_agent("probed-binary", "9.9.9");
        let capability = probe_agent(&AgentKind::ClaudeCode, &binary, None)
            .expect("an agent that answers --version");
        assert_eq!(
            capability.details.agent().map(|d| d.version.as_str()),
            Some("9.9.9")
        );
    }

    #[test]
    fn a_binary_that_is_not_there_is_not_an_agent() {
        // The over-claim this crate exists to prevent, in the form the config makes reachable: a
        // typo in `agent.binary` used to leave the node advertising the install on `PATH`, so it
        // won bids and then failed at spawn on every one of them.
        assert!(probe_agent(
            &AgentKind::ClaudeCode,
            std::path::Path::new("/definitely/not/here/claude"),
            None
        )
        .is_none());
    }

    #[test]
    fn a_moved_config_directory_is_where_the_credentials_are() {
        // A node whose owner moved the agent's state used to report "not authenticated" and
        // refuse every run — the honest-probe rule producing a dishonest answer, because it was
        // looking in the wrong place. Kept in step with
        // `offload_agent::transcript::config_dir`, which decides the same question for
        // transcripts. Asserted against the pure seam, so this pins the rule rather than
        // agreeing with whatever the environment happens to say.
        let home = std::path::Path::new("/home/j");
        assert_eq!(
            resolve_config_dir(home, Some("/srv/agent-state".into())),
            PathBuf::from("/srv/agent-state")
        );
        assert_eq!(
            resolve_config_dir(home, None),
            PathBuf::from("/home/j/.claude")
        );
        // An empty variable is somebody unsetting it awkwardly.
        assert_eq!(
            resolve_config_dir(home, Some(String::new().into())),
            PathBuf::from("/home/j/.claude")
        );
    }

    #[test]
    fn two_machines_on_one_account_fingerprint_alike() {
        // The property the whole thing exists for, and the one the previous fingerprint could
        // not have: `$USER` and `$HOME` differ between these two, the account does not, and a
        // cap shared by an account is meaningless unless this holds.
        let desktop = scratch("desktop");
        let laptop = scratch("laptop");
        let uuid = "00000000-0000-4000-8000-000000000001";
        write_settings(&desktop, &settings_with(uuid));
        write_settings(&laptop, &settings_with(uuid));

        assert_eq!(
            account_fingerprint(&desktop, Some(std::path::Path::new("/home/a"))),
            account_fingerprint(&laptop, Some(std::path::Path::new("/home/b")))
        );
        assert_eq!(
            account_fingerprint(&desktop, Some(std::path::Path::new("/home/a"))),
            Some(AccountId::of_account(uuid))
        );
    }

    #[test]
    fn two_accounts_on_one_machine_do_not_fingerprint_alike() {
        // One machine, one `$USER`, one `$HOME`, two config directories, two accounts — which
        // is exactly what the old per-machine fingerprint collapsed into a single account.
        let work = scratch("work");
        let personal = scratch("personal");
        write_settings(
            &work,
            &settings_with("00000000-0000-4000-8000-000000000001"),
        );
        write_settings(
            &personal,
            &settings_with("9d2e7a01-3b8c-4f52-8e17-6c4b9a0d3f28"),
        );
        let home = Some(std::path::Path::new("/home/j"));

        assert_ne!(
            account_fingerprint(&work, home),
            account_fingerprint(&personal, home)
        );
    }

    #[test]
    fn a_nominated_state_directory_beats_the_environment() {
        // ADR-0028. What selects the account used to be a fact about whatever shell started the
        // daemon, and the owner had no way to state it. The nominated path wins outright — not
        // "wins if the variable is unset", which would make the answer depend on the environment
        // of a service manager nobody reads.
        let home = std::path::Path::new("/home/nobody");
        let nominated = std::path::Path::new("/home/nobody/.claude-alt");
        assert_eq!(
            claude_config_dir(home, Some(nominated)),
            nominated,
            "the owner's word, whatever the environment of this process says"
        );

        // And with nothing nominated it is the old rule exactly, both halves of it — asserted
        // through the pure seam so the assertion does not depend on the environment of whatever
        // else is running in this process.
        assert_eq!(
            resolve_config_dir(home, Some("/elsewhere/.claude".into())),
            std::path::Path::new("/elsewhere/.claude")
        );
        assert_eq!(
            resolve_config_dir(home, None),
            std::path::Path::new("/home/nobody/.claude")
        );
    }

    #[test]
    fn a_nominated_directory_is_where_the_account_is_read_from() {
        // The point of the path reaching the probe at all: authentication and the account
        // fingerprint are facts about *that* directory. A machine with two logins on it reported
        // whichever one the environment happened to name.
        let work = scratch("two-logins-work");
        let personal = scratch("two-logins-personal");
        write_settings(
            &work,
            r#"{"oauthAccount":{"accountUuid":"11111111-1111-1111-1111-111111111111"}}"#,
        );
        write_settings(
            &personal,
            r#"{"oauthAccount":{"accountUuid":"22222222-2222-2222-2222-222222222222"}}"#,
        );
        let absent = Some(std::path::Path::new("/nonexistent-home"));
        assert_ne!(
            account_fingerprint(&work, absent),
            account_fingerprint(&personal, absent),
            "two logins on one machine are two accounts and must not share a rate limit"
        );
        assert_eq!(
            account_fingerprint(&work, absent),
            Some(AccountId::of_account(
                "11111111-1111-1111-1111-111111111111"
            )),
        );
    }

    #[test]
    fn an_account_that_cannot_be_identified_is_no_account_at_all() {
        // The probe's standing rule: an unverifiable fact is `None`, never something
        // plausible. A fingerprint invented here would be counted against another node's
        // account cap, which is a wrong answer that wins bids.
        let dir = scratch("nameless");
        let absent = Some(std::path::Path::new("/nonexistent-home"));

        write_settings(&dir, "{}");
        assert_eq!(account_fingerprint(&dir, absent), None);

        write_settings(&dir, r#"{"oauthAccount":{"emailAddress":"a@b.c"}}"#);
        assert_eq!(account_fingerprint(&dir, absent), None);

        write_settings(&dir, r#"{"oauthAccount":{"accountUuid":"  "}}"#);
        assert_eq!(account_fingerprint(&dir, absent), None);

        write_settings(&dir, "not json at all {{{");
        assert_eq!(account_fingerprint(&dir, absent), None);

        std::fs::remove_file(dir.join(".claude.json")).expect("remove");
        assert_eq!(account_fingerprint(&dir, absent), None);
    }

    #[test]
    fn the_settings_file_is_looked_for_in_both_places_it_lands() {
        // With `$CLAUDE_CONFIG_DIR` set the file is inside the config directory; without it, it
        // sits beside the home directory rather than inside `.claude`. Both were observed on one
        // machine, and checking only one loses the account on half of them.
        let home = scratch("home-only");
        let config = scratch("config-empty");
        let uuid = "9d2e7a01-3b8c-4f52-8e17-6c4b9a0d3f28";
        write_settings(&home, &settings_with(uuid));

        assert_eq!(
            account_fingerprint(&config, Some(home.as_path())),
            Some(AccountId::of_account(uuid))
        );

        // …and **not** when the owner nominated that config directory (ADR-0028), which is what
        // `None` says. The hedge above is right for a path this crate guessed at and wrong for
        // one somebody wrote down: a nominated directory with no settings in it is a login
        // nobody has made, and answering with `$HOME`'s account reports the node as the account
        // its owner deliberately did not choose. Found by walking it, on a daemon pointed at a
        // fresh state directory that reported the personal login from a file two levels up.
        assert_eq!(
            account_fingerprint(&config, None),
            None,
            "a nominated directory answers for itself or not at all"
        );
    }

    #[test]
    fn an_unauthenticated_agent_advertises_no_models_and_one_role() {
        // Guards the rule that matters: never claim a capability that will fail at run time.
        let capability = Capability::agent(
            AgentKind::ClaudeCode,
            AgentDetails {
                version: "2.10.0".into(),
                models: Vec::new(),
                max_concurrent: DEFAULT_MAX_CONCURRENT,
            },
            false,
        );
        assert!(!capability.authenticated);
        assert!(capability
            .details
            .agent()
            .expect("agent details")
            .models
            .is_empty());
        // An agent hosts runs and is not a sink, a trigger, or something a run acts on. Each
        // of those is a different grant (ADR-0011).
        assert_eq!(
            capability.roles,
            BTreeSet::from([offload_core::Role::Execute])
        );
        assert_eq!(capability.id.0, "agent:claude-code");
    }

    #[test]
    fn probing_an_unknown_agent_yields_nothing() {
        assert!(probe_agent(
            &AgentKind::Other("nonesuch".into()),
            std::path::Path::new("true"),
            None
        )
        .is_none());
    }
}
