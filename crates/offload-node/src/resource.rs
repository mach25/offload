//! Things on this device a run may be granted the use of (ADR-0011, `Role::Resource`).
//!
//! The role that is different from the other three, and the difference is one sentence: it is
//! the only one that hands a run something. A sink is reached *on behalf of* a run and a trigger
//! *creates* one; a resource is acted on *by* the agent, with the agent deciding what to do with
//! it. So it is the only role that has to be granted per run rather than implied by placement —
//! a node holding a mailbox does not mean every run that lands there may read mail.
//!
//! Two halves, and they meet at the spawn:
//!
//! * **Nominated, not probed.** The owner declares what this device offers, exactly as they do
//!   for a delivery route, because there is no mail client and no credential store here and a
//!   resource that cannot be verified must not be claimed. What can be verified is that a
//!   program exists.
//! * **Projected into the agent's own protocol**, never into one of ours (ADR-0004). For Claude
//!   Code that is an MCP config at spawn time, carrying exactly the servers this run was
//!   granted — beside `--strict-mcp-config`, which is what makes "exactly" true.
//!
//! And a resource held by **another** node, which ADR-0011 calls the substantial piece hiding
//! behind a small role: the call is forwarded, never the credential. The agent is handed a
//! server whose program is `offloadd` itself, which carries each line of the agent's own
//! protocol to the holder and each answer back. Nothing about the far side travels — not the
//! command, not its environment, not the holder's own name for it — which is the sink rule
//! (ADR-0010) pointed the other way.

use crate::config::{Config, ResourceConfig};
use offload_core::{Capability, Role, Service};

/// This node's nominated resources, as capabilities the fleet can see.
///
/// A resource whose program is missing is advertised **unauthenticated** rather than dropped, for
/// the probe's usual reason: a device that quietly offers nothing looks identical to a device
/// nobody configured, and `Constraint::CanUse` refuses it either way. The difference is that one
/// of them can be explained.
#[must_use]
pub fn resource_capabilities(config: &Config) -> Vec<Capability> {
    let mut out = Vec::new();
    for cfg in &config.resources {
        if cfg.id.trim().is_empty() {
            tracing::warn!(
                command = %cfg.command,
                "ignoring a resource with no id: the id is what a run's grant is projected under"
            );
            continue;
        }
        let (service, access) = match (cfg.service(), cfg.access()) {
            (Ok(service), Ok(access)) => (service, access),
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!(resource = %cfg.id, error = %e, "ignoring a resource");
                continue;
            }
        };
        let found = lookup(&cfg.command);
        if let Some(why) = found.why() {
            tracing::warn!(
                resource = %cfg.id,
                command = %cfg.command,
                %why,
                "this resource cannot be used here; runs asking for it will not place here"
            );
        }
        let mut capability =
            Capability::new(cfg.capability_id(), service, [Role::Resource { access }]);
        capability.authenticated = found.why().is_none();
        capability.identity = cfg.identity.clone().map(offload_core::AccountId);
        // The owner's words or nothing — never the program's path. A peer learns that this
        // device offers email for a run to use, and nothing about how; the command is shown in
        // this node's own `offload status`, where it is the owner reading about their own
        // machine.
        capability.description = cfg.description.clone();
        if let Some(why) = found.why() {
            // The reason after the owner's label: `offload probe` prints this clause as *why*,
            // and with the label alone it read as one. No path — see `Program::why`.
            capability.unusable(why);
        }
        out.push(capability);
    }
    out
}

/// What one run's grant comes to on this node: the servers, and permission to call them.
///
/// **Two halves and one function**, because they are one decision and splitting them is how a
/// grant becomes decorative. Measured, on the first end-to-end run of this feature: projecting
/// only the config gave the agent a tool it could see and was then denied, so `--use email` made
/// a mailbox visible and unusable and the run reported a declined permission request. A grant
/// that grants nothing is worse than no grant, because somebody made a decision and it did not
/// take effect.
///
/// It resolves **here rather than at submit** for the reason the grant names a service in the
/// first place: an id is a node's own name for one of its own things, so the tool pattern a run
/// needs is only knowable on the node that will run it. A migrated run is re-projected against
/// the new holder's resources, which is also why nothing about one ever travels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    /// The agent's own configuration shape, or `None` when nothing was granted — which is not
    /// the same as an empty one. Beside `--strict-mcp-config` they come to the same empty set;
    /// they are kept distinct so the command line says which happened.
    pub mcp: Option<String>,
    /// Permission to call what was just handed over: one `mcp__<id>` pattern per server, which
    /// is the agent's own spelling for "every tool this server offers".
    ///
    /// Whole-server rather than per-tool, and that is the grant's own granularity rather than a
    /// shortcut: the operator said this run may use the mailbox, and nobody submitting a run
    /// knows which tools a node's integration happens to expose. Narrowing further is
    /// `--allow`'s job, which layers over this the way it layers over everything else.
    pub allow: offload_core::ToolAllowlist,
}

/// Where in the fleet a granted service can be reached, decided once per run at spawn.
///
/// A pair rather than a lookup, because the two halves are decided differently and both have to
/// be right at the same moment: what *this* node offers comes from its own config, and what the
/// fleet offers comes from the view — which is a tick stale and perfectly adequate, since being
/// wrong means one refused tool call rather than a misplaced run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reachable {
    /// Services some peer offers, and which peer. Consulted only for what this node does not
    /// hold itself: a local resource is always better than a proxied one, and preferring the
    /// hop would be spending the network on a decision nobody asked for.
    pub elsewhere: std::collections::BTreeMap<Service, offload_core::NodeId>,
}

impl Reachable {
    /// Read the fleet's view for peers offering resources.
    ///
    /// The lowest node id wins when several offer the same service, for `arbiter_for`'s reason:
    /// an arbitrary but *stable* choice, so two legs of one run reach the same mailbox rather
    /// than alternating between two of somebody's accounts.
    #[must_use]
    pub fn in_view(view: &offload_core::ClusterView, me: offload_core::NodeId) -> Reachable {
        let mut elsewhere: std::collections::BTreeMap<Service, offload_core::NodeId> =
            std::collections::BTreeMap::new();
        for node in view.nodes.values() {
            if node.id == me || node.status == offload_core::NodeStatus::Dead {
                continue;
            }
            for capability in node.capabilities.resources() {
                if !capability.authenticated {
                    continue;
                }
                let entry = elsewhere
                    .entry(capability.service.clone())
                    .or_insert(node.id);
                if node.id < *entry {
                    *entry = node.id;
                }
            }
        }
        Reachable { elsewhere }
    }
}

/// Resolve one run's grant against what this node actually offers.
///
/// Matched by **service**, because that is what a grant names. Two resources offering one service
/// are both projected: the owner configured two mailboxes and the run asked for email, so
/// withholding one would be this module deciding which of somebody's accounts they meant.
///
/// Built with `serde_json` rather than a format string, for the reason the ask hook's settings
/// block is: a hand-written JSON string containing a path is a quoting bug with an audience.
#[must_use]
pub fn granted(
    config: &Config,
    granted: &[Service],
    reachable: &Reachable,
    run: offload_core::RunId,
) -> Granted {
    let nothing = Granted {
        mcp: None,
        allow: offload_core::ToolAllowlist::default(),
    };
    if granted.is_empty() {
        return nothing;
    }
    let mut servers = serde_json::Map::new();
    let mut patterns = Vec::new();
    let mut here: Vec<Service> = Vec::new();
    for cfg in &config.resources {
        let Ok(service) = cfg.service() else { continue };
        if !granted.contains(&service) {
            continue;
        }
        here.push(service);
        servers.insert(cfg.id.clone(), server_entry(cfg));
        patterns.push(format!("mcp__{}", cfg.id));
    }
    for service in granted {
        if here.contains(service) || !reachable.elsewhere.contains_key(service) {
            continue;
        }
        // Named by **service**, not by the holder's own id for it — which this node does not
        // know and has no business learning. That also makes the agent-visible tool prefix the
        // same on every machine, so a migrated run's transcript stays readable.
        let id = service.to_string();
        servers.insert(id.clone(), proxy_entry(config, run, service));
        patterns.push(format!("mcp__{id}"));
    }
    if servers.is_empty() {
        // A run granted something nothing in the fleet holds any more. Submission checks for
        // this, so getting here means the holder left between then and now — worth a line
        // rather than a silence, and the run still starts, reaching nothing, which is the same
        // outcome as the grant never having been made.
        tracing::warn!(
            granted = ?granted.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "this run was granted resources this node does not offer; it will reach none of them"
        );
        return nothing;
    }
    Granted {
        mcp: Some(serde_json::json!({ "mcpServers": servers }).to_string()),
        allow: offload_core::ToolAllowlist::parse(&patterns).unwrap_or_default(),
    }
}

/// Where a run's granted MCP configuration is written, so the agent can be handed a path.
#[must_use]
pub fn config_path(state_dir: &std::path::Path, run: offload_core::RunId) -> std::path::PathBuf {
    state_dir.join("mcp").join(format!("{run}.json"))
}

/// Write a run's granted configuration where only this user can read it.
///
/// `0600`, and set on the file rather than left to the umask, because the thing that makes this a
/// file at all is that its contents can be a credential: a resource's `env` carries whatever the
/// owner's integration needs to authenticate, and an argument would be readable by any local user
/// through `/proc/<pid>/cmdline`. It is the run's own name so two legs on one machine cannot read
/// each other's, and it is removed when the leg ends — a leftover is no worse than the node
/// config it was copied from, and there is no reason to leave one.
pub fn write_config(
    state_dir: &std::path::Path,
    run: offload_core::RunId,
    json: &str,
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = config_path(state_dir, run);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Truncating an existing file keeps whatever mode it already had, so the mode is set again
    // afterwards rather than trusted to the create flag. A file this process wrote last time is
    // the common case, and a wrong assumption there would be silent.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    file.write_all(json.as_bytes())?;
    Ok(path)
}

/// Remove a run's granted configuration. Best effort: there is nothing useful to do if it fails.
pub fn forget_config(state_dir: &std::path::Path, run: offload_core::RunId) {
    let path = config_path(state_dir, run);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), error = %e, "could not remove a run's mcp config");
        }
    }
}

/// The agent's own configuration for a resource that is on somebody else's machine.
///
/// The program is `offloadd` itself, by `current_exe`, which is the ask hook's trick and it is
/// the same three reasons: there is nothing to discover, nothing to configure, and no way to end
/// up pointed at a different version of this daemon than the one that spawned the agent.
///
/// **No `env`, and that is the whole point.** A local resource's environment is where the token
/// for the service it reaches lives; a proxied one has none, because the credential stays on the
/// device its owner gave it to and only the call moves (ADR-0011, ADR-0002).
fn proxy_entry(config: &Config, run: offload_core::RunId, service: &Service) -> serde_json::Value {
    let program = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "offloadd".to_string());
    serde_json::json!({
        "command": program,
        "args": [
            "use-resource",
            "--socket", config.socket_path().display().to_string(),
            "--run", run.to_string(),
            "--service", service.to_string(),
        ],
    })
}

fn server_entry(cfg: &ResourceConfig) -> serde_json::Value {
    let env: serde_json::Map<String, serde_json::Value> = cfg
        .env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    serde_json::json!({
        "command": cfg.command,
        "args": cfg.args,
        "env": env,
    })
}

/// What this device has at a nominated command — three answers, not two.
///
/// The one thing a general-purpose machine can verify about a nominated program (ADR-0019 §1),
/// and it is what `authenticated` means on every capability that names a command — a sink, a
/// trigger, a resource, a task. The one resolver for all four: `deliver::resolve` was a second
/// copy with one behaviour this lacked, and two spellings of one fact is how they came apart.
///
/// **Executable, not merely present.** This answered an `Option` from `is_file()`, so a
/// `[[tasks]]` entry
/// naming a file without its execute bit was advertised as usable, won the bid, and failed on the
/// spawn with `Permission denied (os error 13)` — measured on one daemon, with `offload status`
/// calling it fine on the line above. Same over-claim as a missing program, one permission bit in.
///
/// **And the two causes are kept apart**, which the first cut of that fix did not do: an
/// `Option` throws the measurement away, so every report then said *its program was not found*
/// about a file sitting right there with mode 644 — a confidently wrong cause, which is the
/// defect `NoBid::RepoUnavailable` was unpicked for in session seventy-five. Whatever is not
/// measured here cannot be worded anywhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Program {
    /// Here, and this process could execute it.
    Runnable(std::path::PathBuf),
    /// Nothing of that name on this device.
    NotFound,
    /// There, and not executable by this process.
    NotExecutable(std::path::PathBuf),
}

impl Program {
    /// Why this device cannot use it, or `None` if it can.
    ///
    /// A sentence about the *device*, with no path in it, because a capability's description
    /// gossips and ADR-0010's rule is that a peer learns what a device can do and never how. The
    /// callers that are node-local — a trigger's own report, a sink's, a spawn that failed — name
    /// the command themselves, where the owner is reading about their own machine.
    pub(crate) fn why(&self) -> Option<&'static str> {
        match self {
            Program::Runnable(_) => None,
            Program::NotFound => Some("its program is not on this device"),
            Program::NotExecutable(_) => Some("its program is there and is not executable"),
        }
    }

    pub(crate) fn path(self) -> Option<std::path::PathBuf> {
        match self {
            Program::Runnable(path) => Some(path),
            Program::NotFound | Program::NotExecutable(_) => None,
        }
    }

    /// The node-local sentence, which may name the command because it never leaves this node.
    pub(crate) fn refusal(&self, command: &str) -> String {
        match self {
            Program::Runnable(_) => format!("{command}: usable"),
            Program::NotFound => format!("{command}: not found on this device"),
            Program::NotExecutable(_) => {
                format!("{command}: found on this device and not executable")
            }
        }
    }
}

/// Look a nominated command up. See [`Program`].
///
/// A path with a separator in it is a path, absolute or not: looking for `bin/watch` inside each
/// `PATH` entry finds nothing and says "not on this device" about a file that is right there.
pub(crate) fn lookup(command: &str) -> Program {
    let path = std::path::Path::new(command);
    if path.is_absolute() || command.contains(std::path::MAIN_SEPARATOR) {
        return classify(path);
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return Program::NotFound;
    };
    let mut found = Program::NotFound;
    for candidate in std::env::split_paths(&paths).map(|dir| dir.join(command)) {
        match classify(&candidate) {
            // The first runnable one wins, which is what a shell does.
            runnable @ Program::Runnable(_) => return runnable,
            // …and a present-but-unrunnable one is remembered rather than returned, because a
            // later `PATH` entry may hold a real one. Reported only if nothing else turns up,
            // which is the honest answer: something of that name is here and cannot be run.
            not_exec @ Program::NotExecutable(_) if found == Program::NotFound => found = not_exec,
            _ => {}
        }
    }
    found
}

fn classify(path: &std::path::Path) -> Program {
    if !path.is_file() {
        return Program::NotFound;
    }
    if executable(path) {
        Program::Runnable(path.to_path_buf())
    } else {
        Program::NotExecutable(path.to_path_buf())
    }
}

#[cfg(unix)]
fn executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// `cfg(unix)` because the bit is; everywhere else the honest answer is the one the old code gave
/// everywhere, and this daemon does not run there today (its control socket is a unix socket).
/// Said as a `cfg` rather than assumed, so the day it does the omission is visible.
#[cfg(not(unix))]
fn executable(_path: &std::path::Path) -> bool {
    true
}

/// What this node does when a peer's run asks to use one of its resources (ADR-0011).
///
/// The holder's half of the proxied call. Its job is small and its refusals are the interesting
/// part: this is the only place in the system where one device runs a program on another
/// device's say-so, so what it will and will not do is worth being able to read in one screen.
pub struct NodeResources {
    config: std::sync::Arc<Config>,
    /// What this node believes about the fleet's runs, for the one check that is not local.
    cluster: std::sync::Weak<offload_cluster::Cluster>,
}

impl std::fmt::Debug for NodeResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeResources").finish_non_exhaustive()
    }
}

impl NodeResources {
    #[must_use]
    pub fn new(
        config: std::sync::Arc<Config>,
        cluster: &std::sync::Arc<offload_cluster::Cluster>,
    ) -> std::sync::Arc<NodeResources> {
        std::sync::Arc::new(NodeResources {
            config,
            cluster: std::sync::Arc::downgrade(cluster),
        })
    }

    /// Was this run actually granted this service?
    ///
    /// Read from the run record in the view, which is the asker's own gossip and therefore not
    /// proof of anything — a member could publish a record granting itself the world. It is
    /// checked anyway, and the reason is worth being precise about rather than pretending it is
    /// a security boundary: it catches the case that will actually happen, which is a run that
    /// was never granted this reaching for it after a migration or a misconfiguration, and it
    /// makes the refusal say so.
    ///
    /// **What actually bounds this is membership.** The fleet is one person's devices, admitted
    /// by a certificate the fleet key signed; a node inside it that lies about its runs is a
    /// node that has already been enrolled, and the answer to that is `offload revoke`. The
    /// alternative — signing run specs so a holder could verify a grant it did not issue — is a
    /// second trust hierarchy for a fleet that has exactly one, and ADR-0012 rejected that shape
    /// for membership already.
    fn granted_to(&self, run: offload_core::RunId, service: &Service) -> Result<(), String> {
        let Some(cluster) = self.cluster.upgrade() else {
            return Err("this node is no longer in a fleet".into());
        };
        let view = cluster.view();
        let Some(record) = view.runs.get(&run) else {
            return Err(format!(
                "this node knows nothing about run {} — it may have finished, or its record \
                 may not have reached here yet",
                run.short()
            ));
        };
        if !record.spec.resources.contains(service) {
            return Err(format!(
                "run {} was not granted the use of {service}",
                run.short()
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl offload_cluster::Resources for NodeResources {
    async fn open(
        &self,
        from: offload_core::NodeId,
        run: offload_core::RunId,
        service: &Service,
    ) -> Result<Box<dyn offload_cluster::ResourceChannel>, String> {
        self.granted_to(run, service)?;
        let cfg = self
            .config
            .resources
            .iter()
            .find(|cfg| cfg.service().is_ok_and(|s| &s == service))
            .ok_or_else(|| format!("this node offers no {service} for a run to use"))?;
        let found = lookup(&cfg.command);
        let program = found
            .clone()
            .path()
            .ok_or_else(|| found.refusal(&cfg.command))?;

        tracing::info!(
            node = %from.short(),
            run_id = %run,
            resource = %cfg.id,
            "opening a resource for a peer's run"
        );
        ChildChannel::spawn(&program, cfg)
    }
}

/// One nominated server, running, with its stdio turned into lines.
///
/// stdout is drained by a task rather than read on demand, for the reason every other pump in
/// this daemon has one: a child that writes more than a pipe buffer while nobody is reading
/// blocks, and a blocked MCP server looks to the agent exactly like one that is thinking.
struct ChildChannel {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    lines: tokio::sync::mpsc::Receiver<String>,
}

impl ChildChannel {
    fn spawn(
        program: &std::path::Path,
        cfg: &ResourceConfig,
    ) -> Result<Box<dyn offload_cluster::ResourceChannel>, String> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut command = tokio::process::Command::new(program);
        command
            .args(&cfg.args)
            .envs(cfg.env.clone())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // Its own process group, so tearing this down takes whatever the server shelled out to
        // with it — the same reason a sink's command gets one.
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", cfg.command))?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "the resource's program has no stdout".to_string())?;
        let (tx, lines) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx.send(line).await.is_err() {
                    return;
                }
            }
        });
        Ok(Box::new(ChildChannel {
            child,
            stdin,
            lines,
        }))
    }
}

#[async_trait::async_trait]
impl offload_cluster::ResourceChannel for ChildChannel {
    async fn send(&mut self, line: String) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "the resource's program is not accepting input".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn next(&mut self) -> Option<String> {
        self.lines.recv().await
    }

    async fn close(&mut self) {
        // Closing stdin first, because a well-behaved server exits on it and a killed one may
        // leave whatever it was doing half done. The kill is the backstop, not the plan.
        self.stdin.take();
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    /// A stable run id for the tests: what the grant is projected *for*, and the only part of
    /// a proxied entry that varies.
    fn a_run() -> offload_core::RunId {
        offload_core::RunId::from_bytes([7; 16])
    }

    use super::*;

    /// The probe's own rule, met one permission bit in — and then the cause kept apart from it.
    ///
    /// `which` answered `is_file()`, so a `[[tasks]]` entry naming a mode-644 file was advertised
    /// as usable, won its bid and failed the spawn with `Permission denied (os error 13)` —
    /// measured on a daemon, with `offload status` calling it fine on the line above. The first
    /// fix returned an `Option`, which threw the measurement away: every report then said *its
    /// program was not found* about a file sitting right there, which is a confidently wrong
    /// cause and worse than the silence it replaced.
    #[test]
    #[cfg(unix)]
    fn a_program_that_cannot_be_executed_is_not_a_program_this_device_has() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("offload-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("nominated.sh");
        let name = path.display().to_string();
        std::fs::write(&path, "#!/bin/sh\necho hi\n").expect("write");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(lookup(&name), Program::NotExecutable(path.clone()));
        assert_eq!(
            lookup(&name).path(),
            None,
            "present and unrunnable is not `have it`"
        );
        // The two causes are worded apart wherever they are worded at all.
        assert_ne!(lookup(&name).why(), Program::NotFound.why());
        assert!(lookup(&name).refusal(&name).contains("not executable"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(lookup(&name), Program::Runnable(path.clone()));
        assert_eq!(lookup(&name).path(), Some(path.clone()));
        assert_eq!(lookup(&name).why(), None);

        // A directory is not a program either, which `is_file()` already answered and which the
        // new check must not have quietly stopped answering.
        assert_eq!(lookup(&dir.display().to_string()), Program::NotFound);
        assert_eq!(
            lookup("no-such-program-anywhere-on-this-box"),
            Program::NotFound
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    fn resource(id: &str, service: &str, command: &str) -> ResourceConfig {
        ResourceConfig {
            id: id.into(),
            service: service.into(),
            command: command.into(),
            ..ResourceConfig::default()
        }
    }

    fn config(resources: Vec<ResourceConfig>) -> Config {
        Config {
            resources,
            ..Config::default()
        }
    }

    #[test]
    fn a_nominated_resource_is_advertised_as_something_a_run_could_be_granted() {
        let caps = resource_capabilities(&config(vec![ResourceConfig {
            access: Some("read-write".into()),
            ..resource("mail", "email", "/bin/sh")
        }]));
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].id.0, "resource:mail");
        assert!(caps[0].is_resource());
        assert!(caps[0].plays(Role::Resource {
            access: offload_core::Access::ReadWrite
        }));
        assert!(caps[0].authenticated, "/bin/sh exists");
        assert!(
            !caps[0].description.contains("/bin/sh"),
            "a peer learns that this device offers email, never the program behind it"
        );
    }

    #[test]
    fn a_resource_whose_program_is_missing_is_advertised_and_refused_rather_than_hidden() {
        // The probe's rule. A device that quietly offers nothing is indistinguishable from a
        // device nobody configured, and only one of those can be explained to somebody asking
        // why their run will not place.
        let caps = resource_capabilities(&config(vec![resource(
            "mail",
            "email",
            "/nowhere/mail-mcp",
        )]));
        assert_eq!(caps.len(), 1);
        assert!(!caps[0].authenticated);

        let mut all = offload_core::Capabilities::empty(
            offload_core::Os::Linux,
            offload_core::Arch::X86_64,
            offload_core::DeviceClass::Laptop,
        );
        all.add(caps[0].clone());
        assert!(!offload_core::Constraint::CanUse {
            service: Service::Email
        }
        .matches(&all));
    }

    #[test]
    fn a_run_granted_nothing_is_handed_no_config_at_all() {
        // Not an empty one. With `--strict-mcp-config` they mean the same thing to the agent, and
        // different things to whoever is reading the command line afterwards.
        let g = granted(
            &config(vec![resource("mail", "email", "/bin/sh")]),
            &[],
            &Reachable::default(),
            a_run(),
        );
        assert_eq!(g.mcp, None);
        assert!(g.allow.is_empty());
    }

    #[test]
    fn a_run_is_handed_exactly_what_it_was_granted_and_may_call_it() {
        // Both halves in one assertion on purpose. The first end-to-end run of this feature
        // projected the config alone: the agent saw `mcp__mail__read_inbox`, was denied it, and
        // reported a declined permission request. A grant that grants nothing is worse than no
        // grant, because somebody made a decision and it did not take effect.
        let config = config(vec![
            resource("mail", "email", "/bin/sh"),
            resource("cal", "calendar", "/bin/sh"),
        ]);
        let g = granted(&config, &[Service::Email], &Reachable::default(), a_run());
        let json = g.mcp.expect("granted one");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let servers = parsed["mcpServers"].as_object().expect("servers");
        assert_eq!(servers.len(), 1, "the calendar was not granted");
        assert!(servers.contains_key("mail"));
        assert_eq!(servers["mail"]["command"], "/bin/sh");

        assert!(g.allow.covers("mcp__mail__read_inbox", ""));
        assert!(
            !g.allow.covers("mcp__cal__add_event", ""),
            "permission follows the grant, not the config file"
        );
    }

    #[test]
    fn two_resources_offering_one_service_are_both_granted() {
        // The owner configured two mailboxes and the run asked for email. Picking one would be
        // this module deciding which of somebody's accounts they meant.
        let config = config(vec![
            resource("work", "email", "/bin/sh"),
            resource("home", "email", "/bin/sh"),
        ]);
        let g = granted(&config, &[Service::Email], &Reachable::default(), a_run());
        let json = g.mcp.expect("granted");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["mcpServers"].as_object().expect("servers").len(), 2);
        assert_eq!(g.allow.patterns().len(), 2);
    }

    #[test]
    fn a_granted_configuration_is_readable_only_by_this_user() {
        // What makes it a file rather than an argument: `env` on a resource is where a
        // credential for the service lives, and a command line is readable by any local user
        // through `/proc/<pid>/cmdline` on a default Linux.
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("offload-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let run = offload_core::RunId::from_bytes([4; 16]);

        let path = write_config(&dir, run, r#"{"mcpServers":{}}"#).expect("write");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);

        // And again over a file that already exists with the wrong mode, because truncating one
        // keeps whatever mode it had and that would be a silent way to lose this.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let path = write_config(&dir, run, r#"{"mcpServers":{}}"#).expect("rewrite");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);

        forget_config(&dir, run);
        assert!(!path.exists());
        // Removing one that is not there is not an error: a run granted nothing never wrote one.
        forget_config(&dir, run);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_grant_this_node_cannot_honour_hands_over_nothing() {
        // Placement should have stopped it. The run starts anyway and reaches nothing, which is
        // the same outcome as the grant never having been made — the safe direction, and in
        // particular it must not hand out permission for a server that is not there.
        let g = granted(
            &config(Vec::new()),
            &[Service::Email],
            &Reachable::default(),
            a_run(),
        );
        assert_eq!(g.mcp, None);
        assert!(g.allow.is_empty());
    }

    fn peer_offering_as(seed: u8, service: &str, authenticated: bool) -> offload_core::NodeView {
        let mut caps = offload_core::Capabilities::empty(
            offload_core::Os::Linux,
            offload_core::Arch::X86_64,
            offload_core::DeviceClass::Phone,
        );
        let mut capability = offload_core::Capability::new(
            format!("resource:{service}"),
            service.parse::<Service>().expect("service"),
            [offload_core::Role::Resource {
                access: offload_core::Access::Read,
            }],
        );
        capability.authenticated = authenticated;
        caps.add(capability);
        offload_core::NodeView::new(
            offload_core::NodeId::from_bytes([seed; 32]),
            caps,
            offload_core::WorkPolicy::for_class(offload_core::DeviceClass::Phone),
            offload_core::Millis(0),
        )
    }

    fn peer_offering(seed: u8, service: &str) -> offload_core::NodeView {
        peer_offering_as(seed, service, true)
    }

    #[test]
    fn a_service_only_a_peer_offers_is_projected_as_a_proxy() {
        // ADR-0011's motivating case: the phone holds the mailbox and hosts nothing, so the
        // desktop runs the agent and forwards the call. The agent gets a server whose program
        // is this daemon, and — the part that matters — **no environment**, because the
        // credential stays on the device its owner gave it to.
        let me = offload_core::NodeId::from_bytes([1; 32]);
        let mut view = offload_core::ClusterView::new(me);
        view.upsert_node(peer_offering(2, "email"));
        let reachable = Reachable::in_view(&view, me);

        let g = granted(&config(Vec::new()), &[Service::Email], &reachable, a_run());
        let json: serde_json::Value =
            serde_json::from_str(&g.mcp.expect("a server")).expect("json");
        let server = &json["mcpServers"]["email"];
        assert!(server["command"].is_string());
        assert!(
            server["env"].is_null(),
            "a proxied resource carries no credential: {server}"
        );
        let args: Vec<String> = serde_json::from_value(server["args"].clone()).expect("args");
        assert!(args.contains(&"use-resource".to_string()));
        assert!(args.contains(&a_run().to_string()));

        // And permission to call it, which is the half a grant is worthless without.
        assert!(g.allow.covers("mcp__email__read_inbox", ""));
    }

    #[test]
    fn a_resource_this_node_holds_itself_is_never_proxied() {
        // A local one is always better: the hop costs a round trip through the mesh, and
        // preferring it would be spending the network on a decision nobody asked for.
        let me = offload_core::NodeId::from_bytes([1; 32]);
        let mut view = offload_core::ClusterView::new(me);
        view.upsert_node(peer_offering(2, "email"));
        let reachable = Reachable::in_view(&view, me);

        let g = granted(
            &config(vec![resource("mail", "email", "/bin/sh")]),
            &[Service::Email],
            &reachable,
            a_run(),
        );
        let json: serde_json::Value =
            serde_json::from_str(&g.mcp.expect("a server")).expect("json");
        let servers = json["mcpServers"].as_object().expect("servers");
        assert_eq!(servers.len(), 1, "one mailbox, not two: {servers:?}");
        assert!(servers.contains_key("mail"), "the local one, by its own id");
    }

    #[test]
    fn an_unauthenticated_peer_resource_is_not_somewhere_to_reach() {
        // The probe's rule on the other plane: a resource whose program is missing is
        // advertised anyway so it can be explained, and offering to proxy to it would turn an
        // explainable configuration error into a tool that hangs.
        let me = offload_core::NodeId::from_bytes([1; 32]);
        let mut view = offload_core::ClusterView::new(me);
        view.upsert_node(peer_offering_as(2, "email", false));
        assert!(Reachable::in_view(&view, me).elsewhere.is_empty());
    }

    #[test]
    fn the_lowest_id_wins_when_two_devices_offer_the_same_service() {
        // Arbitrary but *stable*, for `arbiter_for`'s reason: two legs of one run must reach
        // the same mailbox rather than alternating between two of somebody's accounts.
        let me = offload_core::NodeId::from_bytes([9; 32]);
        let mut view = offload_core::ClusterView::new(me);
        view.upsert_node(peer_offering(5, "email"));
        view.upsert_node(peer_offering(3, "email"));
        let reachable = Reachable::in_view(&view, me);
        assert_eq!(
            reachable.elsewhere.get(&Service::Email),
            Some(&offload_core::NodeId::from_bytes([3; 32]))
        );
    }
}
