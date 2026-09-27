//! Getting a notification to a person (ADR-0010).
//!
//! The delivery plane's impure half. `offload_core::notify` decides *what* is worth telling
//! somebody and the store remembers *whether it was told*; this runs the commands and keeps the
//! two honest.
//!
//! The shape is an outbox, and every property that matters comes from that choice:
//!
//! * **Nothing is emitted as a side effect.** A pass reads the event log forward from each
//!   sink's cursor, writes down what it noticed, and only then tries to send. A daemon killed
//!   between the two does the same thing again on start, because the outbox row is the memory
//!   and the log is the source.
//! * **Delivery never blocks a run.** This is a tick of its own, beside the lease heartbeat —
//!   not something a turn boundary or a checkpoint waits on. A sink can be slow or gone in ways
//!   a run cannot afford to care about (ADR-0010's last consequence).
//! * **A broken sink accumulates work rather than swallowing it.** The cursor advances for
//!   everybody; the retry comes from the table. That is the whole reason those are two
//!   mechanisms.
//! * **Giving up is loud and recorded.** After [`MAX_ATTEMPTS`] a notification stops being
//!   retried and the reason stays in the row, because a delivery plane that cannot explain a
//!   silence has failed at its only job.
//!
//! **A route may be on another node**, which is the point of the whole plane: the run finishes on
//! the desktop and the phone — which cannot host an agent at all — is the thing that reaches the
//! person. Two rules make that safe, and both are the same rule pointing in opposite directions:
//!
//! * **The sender keeps the outbox.** The node that logged the event remembers who it has told,
//!   because it is the only node that knows. A peer is asked, answers, and forgets; being asked
//!   twice is the at-least-once contract working.
//! * **Nothing about a route travels.** A peer is told *what* to say and never *how* it says it —
//!   the command, its arguments and its credentials stay on the device that holds them. The asker
//!   knows only the id of a capability that device advertises.
//!
//! What is still deliberately missing is the *audience*: every authenticated route in the fleet
//! is used, rather than the one a run asked for. `Constraint::HasService` is already the right
//! question and a notification cannot yet name one.

use crate::config::{Config, SinkConfig};
use offload_core::{notify, LogEvent, Millis, Notification, Topic};
use std::collections::BTreeMap;
use std::sync::Arc;

/// How many times to try before giving up on one notification.
///
/// Small on purpose. The failures a retry fixes here are transient — a laptop between networks,
/// a script that was being edited — and the failures it does not fix are a command that is not
/// there, which no amount of trying will conjure. Three tries spread over three ticks, then the
/// row keeps its reason so somebody can find out why their phone stayed quiet.
pub const MAX_ATTEMPTS: u32 = 3;

/// How many log entries one pass looks at per sink, and how many sends it makes.
///
/// Bounded so a node that has been offline for a week does not spend its first tick delivering
/// a fortnight of history in one breath. Whatever is left is still in the outbox next tick.
const PER_PASS: u32 = 32;

/// A route from a run to a human, as the daemon sees it.
///
/// A trait for the reason `offload-agent` is one: it keeps the thing that decides separate from
/// the thing that does, so a test can answer without a subprocess and so the second
/// implementation — push, a webhook — is an addition rather than an edit.
#[async_trait::async_trait]
pub trait Sink: Send + Sync + 'static {
    /// This node's name for the route. The dedup key's third component.
    fn id(&self) -> &str;

    /// Can it be used at all? The probed half of ADR-0010's "never over-claimed": a route whose
    /// program is missing is `authenticated: false`, and a node that claimed it anyway would win
    /// the routing decision and then drop the message.
    fn usable(&self) -> Result<(), String>;

    /// What kind of route this is, so a run can ask for one by name (ADR-0010's audience).
    ///
    /// `None` for a route whose owner did not say — the service is a *declaration* and this node
    /// cannot infer it from a command. Such a route still carries the news nobody routed
    /// (`Audience::Everyone`) and cannot be asked for by service, which is the same honesty rule
    /// the auth flag follows: what cannot be established is not claimed.
    fn service(&self) -> Option<&offload_core::Service>;

    async fn deliver(&self, note: &Notification) -> Result<(), String>;
}

/// A sink that runs a program the owner nominated.
///
/// The only sink a general-purpose machine can honestly offer today, and the one that makes
/// every other route reachable through two lines of shell. Three things it is careful about:
///
/// * **No templating**, so there is no quoting bug to have. The notification arrives three ways
///   — JSON on stdin for anything that wants structure, `OFFLOAD_*` variables for a shell
///   one-liner, and the summary as a final argument for `notify-send`-shaped programs.
/// * **No inherited stdin, and output is not the run's.** A sink's chatter belongs in the
///   daemon's log, never in the event stream it was derived from.
/// * **Bounded.** A script that hangs must not stall the tick behind it, so it is given
///   [`Self::TIMEOUT`] and then killed. A sink cannot be trusted with the daemon's liveness.
#[derive(Debug)]
pub struct ExecSink {
    id: String,
    command: String,
    args: Vec<String>,
    /// What this device has at [`Self::command`], resolved once at construction, and the answer
    /// is what `authenticated` reports: "the program is there and can be run" is the only part
    /// of a delivery route this node can verify, so it is the part it must actually verify.
    ///
    /// Three answers, not two, so a route refused because its program is *there and not
    /// executable* does not report it as missing. `resource::lookup` is the one resolver for
    /// every nominated program in this daemon; this was a second copy of it, and the two had
    /// drifted in both directions — that one did not understand a relative path with a separator
    /// in it, and neither asked whether the file could be executed at all.
    resolved: crate::resource::Program,
    /// What its owner declared it to be. Unreadable spellings are `None` rather than an error:
    /// the same route was already delivering before a run could ask for one by service, and a
    /// misspelling must not silently stop it.
    service: Option<offload_core::Service>,
}

impl ExecSink {
    /// Long enough for a `curl` to a phone-notification service on a bad connection, short
    /// enough that a hung script costs one tick rather than the evening.
    pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

    #[must_use]
    pub fn new(config: &SinkConfig) -> Self {
        ExecSink {
            id: config.id.clone(),
            command: config.command.clone(),
            args: config.args.clone(),
            resolved: crate::resource::lookup(&config.command),
            service: config.service().ok(),
        }
    }
}

/// What a sink's program is handed on stdin.
///
/// A shape of its own rather than `Notification`'s serde form, because this one is an
/// **interface**: somebody's two-line script parses it, so it has to be stable and it has to be
/// readable. `RunId` encodes as an array of bytes on the wire — right for a protocol, unusable
/// in `jq` — and `kind` is the stable name rather than a variant identifier, so renaming a
/// variant cannot silently change what a `case` statement sees.
///
/// The rendered `title` and `summary` are here as well as the structured `notice`, so that the
/// simplest possible script needs no formatting logic and a more careful one loses nothing.
#[derive(Debug, serde::Serialize)]
struct Payload<'a> {
    /// The run this is about, or `null` for news about the fleet itself (ADR-0012). Kept as
    /// the same field rather than replaced: this JSON is an interface a person's script reads,
    /// and a rename would break every one of them for the sake of a tidier name.
    run: Option<String>,
    /// `run` or `fleet`. What `run` being null means, said rather than implied.
    about: &'static str,
    seq: u64,
    at_unix_ms: u64,
    kind: &'static str,
    title: String,
    summary: String,
    notice: &'a offload_core::Notice,
}

impl<'a> Payload<'a> {
    fn of(note: &'a Notification) -> Self {
        Payload {
            run: note.subject.as_run().map(|run| run.to_string()),
            about: note.subject.topic().name(),
            seq: note.seq,
            at_unix_ms: note.at.0,
            kind: note.kind_name(),
            title: note.title(),
            summary: note.summary(),
            notice: &note.notice,
        }
    }
}

#[async_trait::async_trait]
impl Sink for ExecSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn usable(&self) -> Result<(), String> {
        match self.resolved {
            crate::resource::Program::Runnable(_) => Ok(()),
            _ => Err(self.resolved.refusal(&self.command)),
        }
    }

    fn service(&self) -> Option<&offload_core::Service> {
        self.service.as_ref()
    }

    async fn deliver(&self, note: &Notification) -> Result<(), String> {
        let program = match &self.resolved {
            crate::resource::Program::Runnable(path) => path,
            // The same message `usable` gives, because this is reachable: a script can be
            // deleted — or have its execute bit taken off — between the probe and the send.
            other => return Err(other.refusal(&self.command)),
        };

        let payload = serde_json::to_string(&Payload::of(note)).map_err(|e| e.to_string())?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(&self.args)
            .arg(note.summary())
            .env(
                "OFFLOAD_RUN",
                note.subject
                    .as_run()
                    .map(|run| run.to_string())
                    .unwrap_or_default(),
            )
            .env("OFFLOAD_ABOUT", note.subject.topic().name())
            .env("OFFLOAD_TITLE", note.title())
            .env("OFFLOAD_SUMMARY", note.summary())
            .env("OFFLOAD_SINK", &self.id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Its own process group, so a timeout takes whatever the script shelled out to with it.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            // A sink that does not read stdin is normal, so a broken pipe here is not a
            // failure — only the exit status is.
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let finished = tokio::time::timeout(Self::TIMEOUT, child.wait_with_output()).await;
        let output = match finished {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => {
                return Err(format!(
                    "{} did not finish within {}s",
                    self.command,
                    Self::TIMEOUT.as_secs()
                ))
            }
        };

        if output.status.success() {
            return Ok(());
        }
        // The script's own words, because they are the whole diagnosis: "exit 1" tells nobody
        // whether the token was wrong or the network was down.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim().lines().next_back().unwrap_or("no output");
        Err(format!(
            "{} exited {}: {detail}",
            self.command,
            output
                .status
                .code()
                .map_or_else(|| "on a signal".to_string(), |c| c.to_string())
        ))
    }
}

/// Somewhere a notification can go: a route on this node, or one a peer advertises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// `None` for a route on this node.
    pub node: Option<offload_core::NodeId>,
    /// Where a notice sent here *goes*: to a person, or to one of this node's own rules.
    ///
    /// The second is ADR-0057's whole change to this plane, and it is a field rather than a
    /// convention over [`Self::id`] because the difference decides two things a string cannot be
    /// trusted with: whether the run's `Audience` applies, and whether a machine-started run's
    /// news may go there at all.
    pub fires: Option<offload_core::RuleId>,
    /// The holding node's own name for it — the `id` from its `[[sinks]]` entry, which is what
    /// its capability is advertised under.
    pub id: String,
    /// What the route claims to be, which is what a run's audience is matched against.
    ///
    /// `None` only for a local route whose owner did not spell a service out: a peer's route
    /// arrives as a capability, and a capability without a service is not one.
    pub service: Option<offload_core::Service>,
    /// What to call the holder when explaining something to a person.
    pub node_name: String,
    /// Is the holder answering right now?
    ///
    /// A route that is away is still a route: its notifications are queued and wait for it,
    /// indefinitely, because "you will be told when you pick your phone up" is the promise. What
    /// `false` changes is only that nothing is *attempted* — and that a failure is not counted
    /// against the retry budget, since a device that never heard the ask did not refuse it.
    pub reachable: bool,
    /// What the holder says is wrong with the route itself, when something is.
    ///
    /// The third state between working and gone, and it needs to exist for the same reason
    /// `reachable` does: a peer whose credential does not verify is a route that **is listed**
    /// and cannot carry anything at the moment. Treating that as *gone* threw away everything
    /// already queued for it — with `offload sinks` still showing the route on the next line —
    /// and recorded the reason as "this route no longer exists in the fleet", which was not
    /// true. Nothing is attempted and nothing is spent, exactly as for a device that is away.
    ///
    /// `None` for this node's own routes even when they are broken, which is deliberate and is
    /// the asymmetry `offload-probe` has: a peer is believed about itself, and our own missing
    /// script is a thing we can try, fail at, and be loud about.
    pub unusable: Option<String>,
}

impl Route {
    /// The outbox key. Qualified by node, so two devices that both call a route `phone` are two
    /// routes and not one — which they are, and conflating them would mean telling one person
    /// twice while never telling the other.
    ///
    /// The **full** node id, not an abbreviation: this string is an identity that decides whether
    /// something has already been sent, and a truncated key is a collision waiting for two
    /// devices whose keys share a prefix.
    #[must_use]
    pub fn key(&self) -> String {
        match self.node {
            Some(node) => format!("{node}/{}", self.id),
            None => self.id.clone(),
        }
    }

    /// How to say which route this is, in one column.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.node {
            Some(_) => format!("{}/{}", self.node_name, self.id),
            None => self.id.clone(),
        }
    }
}

/// A sink's own id, recovered from the capability it is advertised under.
///
/// One place, because it is a convention (`sink:<id>`) rather than a parse: [`sink_capabilities`]
/// writes the prefix and this reads it. Two copies of a convention is one copy that will be
/// wrong.
#[must_use]
pub fn sink_id_of(capability: &str) -> &str {
    capability.strip_prefix("sink:").unwrap_or(capability)
}

/// The fleet, as the delivery plane needs it: which routes exist elsewhere, and how to use one.
///
/// A trait so this module does not depend on the mesh, and so a test can answer both questions
/// without a network — the same shape `Peers` gives the supervisor.
#[async_trait::async_trait]
pub trait Fleet: Send + Sync + 'static {
    /// Routes on *other* nodes that could carry a notification right now.
    ///
    /// Read from gossiped capabilities, so it is a tick stale and it believes what a peer says
    /// about itself: an unauthenticated route is skipped rather than queued, because a node
    /// saying "my credential does not work" is not a delivery to retry. Our *own* broken routes
    /// are queued and given up on loudly, which is the same asymmetry `offload-probe` has —
    /// this node is responsible for its own claims and takes a peer at its word.
    fn remote_routes(&self) -> Vec<Route>;

    /// This node's id, which is what a local route's [`Route::node`] leaves unsaid. `None` if it
    /// cannot be told (the node is shutting down), and then a local route is treated as being on
    /// the run's home, which errs towards telling (ADR-0073).
    fn me(&self) -> Option<offload_core::NodeId>;

    /// Where a node stands, for a queued row whose route is not in [`Fleet::remote_routes`].
    fn standing(&self, node: offload_core::NodeId) -> Standing;

    /// Ask a node to carry it.
    async fn send(
        &self,
        node: offload_core::NodeId,
        sink: &str,
        note: &Notification,
    ) -> Result<(), SendFailure>;
}

/// Where a node stands with the fleet, as far as this node knows — which decides whether a queued
/// row for one of its routes has lost its route or is waiting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// In the view: if its route is not listed, it has stopped offering it.
    Listed,
    /// Not in the view. After a restart that is nearly everybody for a while, so it is not gone.
    Unheard,
    /// Thrown out of the fleet: gone for good.
    Revoked,
}

/// Why a peer did not carry a notification: whether it answered decides whether it was tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendFailure {
    /// No answer at all: the device is away. Its news waits, and no attempt is spent.
    Unanswered(String),
    /// It answered that its route failed. An attempt, counted towards [`MAX_ATTEMPTS`].
    Refused(String),
}

/// What this node does when a notice is *for a rule* rather than for a person (ADR-0057).
///
/// The same seam [`Fleet`] is, and for the same reason: this module's pass takes a store, some
/// routes and a clock, and firing a rule needs the supervisor, the config and the bid round —
/// which live a layer up. So the pass decides *that* a rule is owed a notice, and the caller
/// knows how to submit one.
#[async_trait::async_trait]
pub trait Fires: Send + Sync + 'static {
    /// The notice-bound rules on this node, as routes.
    ///
    /// Read every pass rather than held, because a rule can be written or forgotten between two
    /// passes and a route list that was right a minute ago is how a new rule waits an hour for
    /// its first notice.
    fn rule_routes(&self) -> Vec<Route>;

    /// Fire one, with the notice that fired it. `Err` is a sentence, and the plane retries it
    /// like any other undelivered notification.
    async fn fire(&self, rule: offload_core::RuleId, note: &Notification) -> Result<(), String>;
}

/// This node's sinks, built once from config.
pub struct Sinks {
    sinks: Vec<Arc<dyn Sink>>,
}

impl std::fmt::Debug for Sinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sinks")
            .field("count", &self.sinks.len())
            .finish()
    }
}

impl Sinks {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let sinks = config
            .sinks
            .iter()
            .filter(|cfg| !cfg.id.is_empty())
            .map(|cfg| Arc::new(ExecSink::new(cfg)) as Arc<dyn Sink>)
            .collect();
        Sinks { sinks }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Sink>> {
        self.sinks.iter().find(|sink| sink.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Sink>> {
        self.sinks.iter()
    }
}

/// The capabilities this node's configured routes amount to (ADR-0010, ADR-0011).
///
/// `authenticated` is the probe being honest about the one thing it can check: the program
/// exists. It cannot check that the script reaches a human, which is the same limit an email
/// sink with a working SMTP server and a dead mailbox would have — and it is why the *service*
/// is the owner's declaration while the auth flag is not.
///
/// A sink whose id is empty is dropped with a warning rather than advertised under a blank
/// name: the id is a dedup key, and a blank one would collide with the next blank one.
#[must_use]
pub fn sink_capabilities(config: &Config) -> Vec<offload_core::Capability> {
    let mut out = Vec::new();
    for cfg in &config.sinks {
        if cfg.id.is_empty() {
            tracing::warn!(
                command = %cfg.command,
                "ignoring a sink with no id: the id is how a delivery is deduplicated"
            );
            continue;
        }
        let service = match cfg.service() {
            Ok(service) => service,
            Err(e) => {
                tracing::warn!(sink = %cfg.id, error = %e, "ignoring a sink: unreadable service");
                continue;
            }
        };
        let usable = ExecSink::new(cfg).usable();
        if let Err(reason) = &usable {
            // Loud, because the alternative is a route that looks configured and silently
            // drops everything.
            tracing::warn!(sink = %cfg.id, %reason, "this delivery route cannot be used");
        }
        let mut capability =
            offload_core::Capability::new(cfg.capability_id(), service, [offload_core::Role::Sink]);
        capability.authenticated = usable.is_ok();
        capability.identity = cfg.identity.clone().map(offload_core::AccountId);
        // The owner's words or nothing. It used to default to the program's path, which reads
        // like a helpful fallback and is the one part of a route that never leaves this node:
        // a peer learns that this device *has* a push route, never how it works. The command is
        // still shown where it belongs, in this node's own `offload sinks`.
        capability.description = cfg.description.clone();
        if usable.is_err() {
            // …and then the reason, because `offload probe` prints this clause as *why* and the
            // label alone read as one. Not the route's own error, which names the command: the
            // fleet's copy of a sink deliberately says the device has a route and never how it
            // works, and `offload sinks` on this node prints the error in full.
            // `Program::why` is the same measurement worded without a path; the fallback covers
            // a route that is unusable for some reason other than its program, which is not a
            // state anything can reach today and is answered rather than assumed away.
            capability.unusable(
                crate::resource::lookup(&cfg.command)
                    .why()
                    .unwrap_or("this node cannot use it"),
            );
        }
        out.push(capability);
    }
    out
}

/// What this device is **now**, rather than what it was when the daemon started.
///
/// `deliver::capabilities` shells out — to the agent binary for its version, to `nmcli`, to
/// `/sys` — so nothing calls it per request; the daemon probes on a schedule and everything else
/// reads the answer from here. That much was always true. What was not is who could see the
/// fresh one: the re-probe handed its result to `Cluster::set_capabilities` and nowhere else, so
/// the fleet's copy moved and **the daemon's own copy was frozen for the life of the process**.
///
/// It was harmless while nothing local read it, which is what `main`'s own comment assumed —
/// *"for a single node with no one to tell, once is enough"*. ADR-0046 and ADR-0047 made the
/// local doors read it: the no-cluster submission arm, `offload resume`, and the `accepting` line
/// in `offload status`. Measured on one daemon with the link turned metered underneath it, three
/// commands, seconds apart:
///
/// ```text
/// $ offload status
/// accepting   yes
/// $ offload run --repo … "new work?"
/// Error: no node will take this run
///   solo         network is metered and policy disallows it
/// $ offload resume 01a053607c0a
/// resumed 01a053607c0a
/// ```
///
/// The bid round reads the cluster's copy and refuses; the status line and the resume door read
/// the snapshot and do not. One fact, three readers, two of them thirteen seconds stale and
/// staying that way.
///
/// A `Mutex<Arc<..>>` rather than a lock held across the read: every caller wants a whole
/// consistent `Capabilities`, and cloning the `Arc` is what keeps the probe from ever waiting on
/// a request handler.
#[derive(Debug)]
pub struct Current(std::sync::Mutex<std::sync::Arc<offload_core::Capabilities>>);

impl Current {
    #[must_use]
    pub fn new(capabilities: offload_core::Capabilities) -> Self {
        Current(std::sync::Mutex::new(std::sync::Arc::new(capabilities)))
    }

    /// What this device is, as of the last probe.
    #[must_use]
    pub fn now(&self) -> std::sync::Arc<offload_core::Capabilities> {
        self.lock().clone()
    }

    /// Store a fresh probe. `true` when it is not what we had, which is only worth a log line —
    /// whether to *gossip* it is `Cluster::set_capabilities`' own question, asked of the fleet's
    /// copy, and this must not become a second answer to it.
    /// Replace what is held, and say which fields changed — empty when nothing did.
    pub fn refresh(&self, capabilities: offload_core::Capabilities) -> Vec<&'static str> {
        let mut held = self.lock();
        let changed = held.differences(&capabilities);
        if !changed.is_empty() {
            *held = std::sync::Arc::new(capabilities);
        }
        changed
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::sync::Arc<offload_core::Capabilities>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The model list this node's agent last reported (ADR-0080), kept beside the probe.
///
/// Not probed with everything else: the read starts the agent for a couple of seconds, and its
/// answer changes only when the agent's version or login does, or when a new model has been
/// released and somebody asks. So `read_models` in the daemon reads on its own cadence, stores
/// it here, and nudges `reprobe`, which stays the one writer of capabilities (ADR-0048) and folds
/// this in through [`capabilities`].
#[derive(Debug, Default)]
pub struct Models {
    held: std::sync::Mutex<Option<ModelsRead>>,
    /// A person on this node asked for a fresh read.
    asked: tokio::sync::Notify,
    /// What is held changed, so the capabilities should be folded again.
    changed: tokio::sync::Notify,
}

/// One reading of the agent's model list, and which agent it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelsRead {
    /// The agent's version when it was read: a list read from one version is not claimed for
    /// another, since a new model arrives with a new agent.
    pub version: String,
    /// Whether it was logged in, for the same reason: the list is the account's.
    pub authenticated: bool,
    pub models: Vec<offload_core::Model>,
}

impl Models {
    /// What is held, if anything has been read.
    #[must_use]
    pub fn held(&self) -> Option<ModelsRead> {
        self.lock().clone()
    }

    /// Replace what is held, and wake `reprobe` if it changed. Returns whether it did.
    pub fn store(&self, read: ModelsRead) -> bool {
        let mut held = self.lock();
        if held.as_ref() == Some(&read) {
            return false;
        }
        *held = Some(read);
        drop(held);
        self.changed.notify_one();
        true
    }

    /// A person here asked for the list to be read again.
    pub fn ask(&self) {
        self.asked.notify_one();
    }

    /// Resolves when somebody here asks.
    pub async fn asked(&self) {
        self.asked.notified().await;
    }

    /// Resolves when what is held has changed.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }

    /// The list for `version` and `authenticated`, or nothing: a reading of another version or
    /// another login is not this agent's list.
    fn for_agent(&self, version: &str, authenticated: bool) -> Vec<offload_core::Model> {
        self.lock()
            .as_ref()
            .filter(|read| read.version == version && read.authenticated == authenticated)
            .map(|read| read.models.clone())
            .unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<ModelsRead>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Ask the agent for its model list, now, as the agent a run would get (ADR-0080). Blocking.
///
/// # Errors
///
/// Whatever the agent's answer was instead of a list; see `ClaudeCode::models`.
pub fn read_models(config: &Config) -> Result<Vec<offload_core::Model>, offload_agent::AgentError> {
    crate::supervisor::agent_for(config).models()
}

/// [`capabilities`] with the model list read now, blocking, for a command with no daemon to
/// have read it: `offload probe --config` reports what the daemon would advertise, and that
/// includes the agent's own list (ADR-0080). A list that cannot be read is left out, as the
/// daemon would leave it out.
#[must_use]
pub fn capabilities_reading_models(config: &Config) -> offload_core::Capabilities {
    let models = Models::default();
    let first = capabilities(config, &models);
    if let Some(agent) = first
        .agent(&offload_core::AgentKind::ClaudeCode)
        .filter(|agent| agent.authenticated)
    {
        if let Ok(list) = read_models(config) {
            models.store(ModelsRead {
                version: agent
                    .details
                    .agent()
                    .map(|details| details.version.clone())
                    .unwrap_or_default(),
                authenticated: true,
                models: list,
            });
            return capabilities(config, &models);
        }
    }
    first
}

/// Put the held model list into the agent's capability — only for the agent it was read from,
/// and only while logged in, because an unauthenticated agent can reach nothing.
fn with_models(capabilities: &mut offload_core::Capabilities, models: &Models) {
    let kind = offload_core::AgentKind::ClaudeCode;
    let Some(mut capability) = capabilities.agent(&kind).cloned() else {
        return;
    };
    let authenticated = capability.authenticated;
    if let offload_core::capability::ServiceDetails::Agent(details) = &mut capability.details {
        details.models = if authenticated {
            models.for_agent(&details.version, authenticated)
        } else {
            Vec::new()
        };
    }
    capabilities.add(capability);
}

/// What this node is and has: probed with the agent it will actually spawn, plus what its owner
/// nominated.
///
/// One function because the two halves used to be written out at each of the daemon's two probe
/// sites, and the probe half was wrong at both: it asked `claude` on `PATH` while the supervisor
/// spawns `agent.binary`, so a node whose owner said where their agent is advertised a different
/// program's version and authentication — or, with nothing named `claude` on `PATH`, advertised
/// no agent at all and never bid, on a machine whose configured agent runs perfectly. The binary
/// is a fact only this crate holds, which is exactly why the joining happens here.
#[must_use]
pub fn capabilities(config: &Config, models: &Models) -> offload_core::Capabilities {
    let mut probed = offload_probe::probe_with(
        &config.agent.binary,
        config.agent.config_dir.as_deref(),
        config.metered,
    );
    // Where this daemon's worktrees and blobs go, not wherever it happened to be started from.
    probed.disk_free_mb = offload_probe::free_disk_under(&config.state_dir);
    if let Some(facts) = host_facts(&config.state_dir, std::time::SystemTime::now()) {
        facts.apply(&mut probed, config.metered.is_some());
    }
    if let Some(sustains) = config.agent.max_concurrent {
        set_agent_concurrency(&mut probed, sustains.0);
    }
    with_models(&mut probed, models);
    if let Some(wanted) = &config.agent.account {
        hold_the_agent_to_one_account(&mut probed, wanted);
    }
    with_nominated(probed, config)
}

/// What a host process told the daemon about the device it runs on (ADR-0066 §3).
///
/// An Android app hosting `offloadd` can ask `BatteryManager` and `ConnectivityManager`, which the
/// daemon cannot: an app is denied `/sys/class/power_supply`, measured on a Samsung phone. So the host
/// writes `host-facts.json` into the state directory and this reads it on every probe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct HostFacts {
    pub battery_percent: Option<u8>,
    pub charging: Option<bool>,
    pub metered: Option<bool>,
    /// Android's thermal status, 0–6 (ADR-0068). Observed pressure, so it goes to the load gate
    /// through [`host_thermal`], not into `Capabilities`.
    #[serde(default)]
    pub thermal: Option<u8>,
    /// `"tablet"` or `"phone"`, by Android's own line (smallest screen width ≥ 600 dp). The probe
    /// cannot tell from inside the app's sandbox and called every Android device a phone —
    /// measured on a Samsung tablet, which `--require class=tablet` then failed to match.
    #[serde(default)]
    pub form: Option<String>,
    /// Whether the screen is on (Android's `PowerManager.isInteractive`), for ADR-0078: a phone on
    /// battery with its screen off asks the fleet to leave it mostly alone. `None` from a host that
    /// does not say, which is never read as "off".
    #[serde(default)]
    pub interactive: Option<bool>,
}

impl HostFacts {
    /// Whether this device should ask to be left mostly alone (ADR-0078): on battery **and** the
    /// screen off, each as the host said. Unknown is not quiet: probing slows only when the phone
    /// says it may, and a host that does not report the screen is treated as one being used.
    #[must_use]
    pub fn quiet(&self) -> bool {
        self.charging == Some(false) && self.interactive == Some(false)
    }
}

/// How hot the host says the device is, while its facts are fresh (ADR-0068), for the three places
/// that read the load average beside it: the bid, `offload status` and `offload policy`. One
/// function, so the report and the decision read the same thing.
#[must_use]
pub fn host_thermal(state_dir: &std::path::Path) -> Option<offload_core::Thermal> {
    host_facts(state_dir, std::time::SystemTime::now())?
        .thermal
        .and_then(offload_core::Thermal::from_android)
}

/// How recent `host-facts.json` has to be to count. The host writes it every 15 s, so this is a
/// few missed writes: long enough that a busy phone is not mistaken for a host that stopped, short
/// enough that a host that *did* stop stops being believed within two probes.
pub const HOST_FACTS_FRESH: std::time::Duration = std::time::Duration::from_secs(120);

/// `host-facts.json` in `state_dir`, if it is there, parses, and was written within
/// [`HOST_FACTS_FRESH`] of `now`. A stale file is no answer at all: the host has stopped telling,
/// and a battery level from ten minutes ago is not good news about now.
#[must_use]
pub fn host_facts(state_dir: &std::path::Path, now: std::time::SystemTime) -> Option<HostFacts> {
    let path = state_dir.join("host-facts.json");
    let written = std::fs::metadata(&path).ok()?.modified().ok()?;
    if now.duration_since(written).unwrap_or_default() > HOST_FACTS_FRESH {
        return None;
    }
    last_host_facts(state_dir)
}

/// `host-facts.json` whatever its age: only for [`HostFacts::quiet`] (ADR-0078). A suspended phone
/// stops its writer along with everything else, so its facts go stale *because* it is set down;
/// [`host_facts`]' freshness rule would read that as "no answer" and wake it back to the ordinary
/// rate at every packet. The host rewrites the file the moment the screen comes on, so the last
/// word being "screen off" is the answer until it is not.
#[must_use]
pub fn last_host_facts(state_dir: &std::path::Path) -> Option<HostFacts> {
    serde_json::from_str(&std::fs::read_to_string(state_dir.join("host-facts.json")).ok()?).ok()
}

impl HostFacts {
    /// Overlay what the host said. Power only where it said something; metered only where the
    /// owner did not nominate it, since a nomination ends that question (ADR-0045 §2).
    pub fn apply(&self, caps: &mut offload_core::Capabilities, metered_nominated: bool) {
        // Only ever between the two handheld classes, which share a policy and a stability:
        // a host saying "tablet" never turns a laptop into one.
        if caps.device_class == offload_core::DeviceClass::Phone
            && self.form.as_deref() == Some("tablet")
        {
            caps.device_class = offload_core::DeviceClass::Tablet;
        }
        match (self.battery_percent, self.charging) {
            (Some(percent), charging) => {
                caps.power = offload_core::PowerSource::Battery {
                    percent: percent.min(100),
                    // A level with no word about the charger is a battery that is not known to
                    // be charging, which is the direction that refuses rather than over-claims.
                    charging: charging.unwrap_or(false),
                };
            }
            (None, Some(true)) => caps.power = offload_core::PowerSource::Ac,
            (None, _) => {}
        }
        if !metered_nominated {
            if let Some(metered) = self.metered {
                caps.metered_network = if metered {
                    offload_core::Metered::Yes
                } else {
                    offload_core::Metered::No
                };
            }
        }
    }
}

/// Refuse to claim an agent that is logged in as somebody the owner did not name (ADR-0028).
///
/// `agent.config_dir` selects the account and this says which one was meant, so the two together
/// are "this device may run agents on my work login and no other". A mismatch is reported as **not
/// authenticated**, which is the state that already means "refuses every run, with a sentence
/// saying why" — rather than as a missing agent, because the program is there and the owner would
/// otherwise go looking for it.
///
/// Deliberately not a startup refusal. A daemon that would not come up could not be asked what it
/// thinks the account is, which is exactly the question somebody has when this fires; and the
/// mismatch is reachable by re-authenticating rather than by editing config, so it is a state to
/// report and recover from rather than a bad configuration.
fn hold_the_agent_to_one_account(capabilities: &mut offload_core::Capabilities, wanted: &str) {
    let kind = offload_core::AgentKind::ClaudeCode;
    let Some(mut capability) = capabilities.agent(&kind).cloned() else {
        return;
    };
    let found = capability.identity.as_ref().map(|id| id.0.clone());
    if found.as_deref() == Some(wanted) {
        return;
    }
    let what = found.unwrap_or_else(|| "no account this node can identify".to_string());
    capability.unusable(&format!(
        "logged in as {what}, and this node is configured for {wanted}"
    ));
    capabilities.add(capability);
}

/// Replace the probe's guess at how many sessions this install sustains with what its owner said.
///
/// Nominated rather than probed for the reason a sink's service is (ADR-0010): the number is a
/// property of a plan and a rate limit, and nothing on the machine states it. The probe's 2 is
/// deliberately low, and it is a **cap**, so leaving an owner unable to raise it made
/// `policy.max_concurrent_runs` decorative above 2 — which is the default on every desktop.
fn set_agent_concurrency(capabilities: &mut offload_core::Capabilities, sustains: u32) {
    let kind = offload_core::AgentKind::ClaudeCode;
    // Only where there is an agent to describe: an owner naming a number for a program that is
    // not there must not conjure the capability, which would be the over-claim `offload-probe`
    // exists to refuse, arriving through the config instead of through the probe.
    let Some(mut capability) = capabilities.agent(&kind).cloned() else {
        return;
    };
    if let offload_core::capability::ServiceDetails::Agent(details) = &mut capability.details {
        details.max_concurrent = sustains;
    }
    capabilities.add(capability);
}

/// Probed capabilities plus everything this node's owner nominated.
///
/// Two sources because they are two kinds of fact and only one of them can be probed: an agent
/// is *discovered* on the machine, and a route to a human — or a resource a run may be granted
/// (ADR-0011), or a watcher that notices something happening (ADR-0020) — is *nominated* by the
/// person whose machine it is. Three of the four roles are the owner's word, which is the sign
/// that the rule is the rule rather than a workaround. Joined here rather than in
/// `offload-probe`, which has no business reading a node's config, and which the CLI's `offload
/// probe` calls with no daemon and no config at all.
#[must_use]
pub fn with_nominated(
    mut capabilities: offload_core::Capabilities,
    config: &Config,
) -> offload_core::Capabilities {
    for capability in sink_capabilities(config) {
        capabilities.add(capability);
    }
    for capability in crate::resource::resource_capabilities(config) {
        capabilities.add(capability);
    }
    for capability in crate::trigger::trigger_capabilities(config) {
        capabilities.add(capability);
    }
    for capability in crate::task::task_capabilities(config) {
        capabilities.add(capability);
    }
    capabilities
}

/// One pass: notice what is new, then send what is waiting.
///
/// Returns how many notifications were delivered, for the caller's log. Errors are handled per
/// item and never abort the pass — one unusable route must not stop another from working, which
/// is the same reasoning that keeps the cursor and the outbox apart.
///
/// `fleet` is `None` on a node with no mesh, which is not a degraded mode: a fleet of one
/// delivering through its own sinks is most of how this project runs.
pub async fn tend_deliveries(
    store: &offload_store::Store,
    sinks: &Sinks,
    fleet: Option<&dyn Fleet>,
    fires: Option<&dyn Fires>,
    now: Millis,
) -> Result<u32, offload_store::StoreError> {
    let routes = all_routes(sinks, fleet, fires);
    if routes.is_empty() {
        return Ok(0);
    }

    // One spec lookup per run rather than per (run, route): both of these are fields on the
    // spec, and a pass with four routes would otherwise read the same record four times.
    let mut audiences: BTreeMap<
        offload_core::RunId,
        (
            offload_core::Audience,
            offload_core::Notices,
            offload_core::Origin,
            Option<offload_core::NodeId>,
        ),
    > = BTreeMap::new();
    // A node with no mesh has no peers, so every run it notices was submitted here.
    let me = fleet.and_then(Fleet::me);

    // Notice first, for every route, so that a send that crashes this process has already been
    // written down as owed.
    for route in &routes {
        let key = route.key();

        // The fleet's own log (ADR-0012 mitigation 4). Every route, with no audience asked:
        // an audience is a field on a *run's* spec, and "a device joined your fleet" is not
        // news any run gets to scope. A person who turned notifications down for their runs has
        // not turned down the alarm that says somebody enrolled a machine.
        let cursor = store.sink_cursor(&key, Topic::Fleet, now)?;
        let mut highest = cursor;
        for (seq, _json) in
            store.fleet_events_to_notice(cursor, notify::NOTABLE_FLEET_KINDS, PER_PASS)?
        {
            highest = highest.max(seq);
            store.notice_delivery(&key, Topic::Fleet, seq, None, now)?;
        }
        if highest > cursor {
            store.advance_sink(&key, Topic::Fleet, highest)?;
        }

        let cursor = store.sink_cursor(&key, Topic::Run, now)?;
        let mut highest = cursor;
        for (seq, run, json, _kind) in
            store.events_to_notice(cursor, notify::NOTABLE_KINDS, PER_PASS)?
        {
            // The cursor advances whatever the audience decides. It bounds the scan and is not
            // the record of what was sent — that is the row — so a route this run's news is not
            // for must not re-read the same entries for ever.
            highest = highest.max(seq);
            // Projected here, and *not* filtered on the `kind` column beside it. The column
            // names the log variant, and `LogKind::Finished` is two pieces of news — the agent
            // reporting success and the agent reporting failure — so a filter over the name
            // cannot tell a watcher's whole reason for existing from the one thing it is meant
            // to be quiet about. Measured before this line existed: a rule failing every firing
            // delivered 0 of 22. The cost is decoding the notable kinds only, which the sender
            // already does for every row it delivers.
            let Some(note) = serde_json::from_str::<offload_core::LogEvent>(&json)
                .ok()
                .and_then(|event| notify::notable(run, seq, &event))
            else {
                // Either the row does not decode under this build, or its kind is scanned for
                // and projects to nothing. Neither is news, and both must move the cursor: a
                // scan that stopped here would re-read the same row for ever.
                tracing::debug!(
                    seq,
                    "a scanned event projects to no notification; skipping it"
                );
                continue;
            };
            let wanted = match audiences.entry(run) {
                std::collections::btree_map::Entry::Occupied(seen) => seen.into_mut(),
                std::collections::btree_map::Entry::Vacant(slot) => {
                    // A missing record means the default audience — everyone — because failing
                    // towards telling somebody is the asymmetry the whole plane is built on: a
                    // duplicate is a nuisance and a silence is the failure.
                    //
                    // It cannot actually happen, and the way it cannot is worth knowing rather
                    // than relying on: a run's events cascade with its row, so an event whose run
                    // is gone is an event that is gone. If that ever changes, this branch does
                    // *not* quietly tell somebody — `notice_delivery` writes a `run_id` with a
                    // foreign key to `runs`, and the insert would fail the whole pass.
                    slot.insert(
                        store
                            .load_run(run)?
                            .map(|run| {
                                (
                                    run.spec.notify,
                                    run.spec.notices,
                                    run.origin,
                                    Some(run.home),
                                )
                            })
                            .unwrap_or_default(),
                    )
                }
            };
            let (audience, notices, origin, home) = (&wanted.0, wanted.1, wanted.2, wanted.3);
            // **A machine's work does not start more of a machine's work** (ADR-0057 §3). A rule
            // bound to `failed` fires an agent run; if that run fails it projects the same
            // notice, and without this the rule fires again for ever, one agent at a time, all
            // night. `Run::origin` is immutable and gossiped, so this is one comparison against
            // a fact nobody has to be asked about.
            //
            // Sinks are unaffected: a person is *told* about a machine-started run, which is
            // most of what the delivery plane is for.
            if route.fires.is_some() && origin != offload_core::Origin::Operator {
                continue;
            }
            // Decided here, at the moment the news is noticed, rather than at the moment it is
            // sent: an outbox row that exists is a promise to deliver, and a route the run never
            // asked for should never be owed one.
            //
            // Two questions, and they are different axes (ADR-0026): *which routes* this run's
            // news is for, and *which news* is worth interrupting anybody with. With only the
            // first, a rule firing every three seconds could say "tell everyone I finished" or
            // "tell nobody anything, including that I failed", and nothing in between.
            // `Audience` selects **routes to people**, and a rule is not one: `--notify nobody`
            // means *do not interrupt anybody about this run*, and reading it as *do not recover
            // this run* would be one axis doing the work of two — the exact mistake ADR-0026 was
            // written to fix. The precedent is ten lines up, where the fleet's own log is
            // offered to every route with no audience asked.
            //
            // `Notices` **is** asked of both, and it costs nothing for the case this is about:
            // `Problems` and `Everything` both admit a failure, so keeping it here means there
            // is one path rather than two.
            let for_this_route = route.fires.is_some() || audience.admits(route.service.as_ref());
            if !for_this_route || !notices.admits(&note.notice) {
                continue;
            }
            // And a third, for routes to people (ADR-0073): that a run *finished* goes only to the
            // device it was submitted from, while its problems go everywhere its audience allows.
            // Every phone with the app is a route, and without this every run anybody started
            // buzzed every device. A rule is not a person and is not asked.
            if route.fires.is_none()
                && home.is_some_and(|home| !notify::reaches(&note.notice, route.node.or(me), home))
            {
                continue;
            }
            store.notice_delivery(&key, Topic::Run, seq, Some(run), now)?;
        }
        if highest > cursor {
            store.advance_sink(&key, Topic::Run, highest)?;
        }
    }

    let mut delivered = 0;
    for pending in store.pending_deliveries(PER_PASS)? {
        let route = routes.iter().find(|route| route.key() == pending.sink);
        let Some(route) = route else {
            // A peer's route this node has not heard of *yet* is waited for, not given up on: a
            // node that has just restarted knows almost nobody until gossip refills its view, and
            // taking that for "no longer exists in the fleet" dropped news for every device that
            // was away at the time (session ninety-four). Gone means revoked, or still listed and
            // no longer offering the route.
            let owner = pending
                .sink
                .split_once('/')
                .and_then(|(node, _)| node.parse::<offload_core::NodeId>().ok());
            let why = match (owner, fleet) {
                (Some(node), Some(fleet)) => match fleet.standing(node) {
                    Standing::Unheard => {
                        store.delivery_deferred(
                            &pending.sink,
                            pending.topic,
                            pending.seq,
                            &format!(
                                "{} has not been heard from since this node started; waiting for it",
                                node.short()
                            ),
                        )?;
                        continue;
                    }
                    Standing::Revoked => "its device has been removed from the fleet",
                    Standing::Listed => "its device no longer offers this route",
                },
                // A sink removed from this node's config, a rule removed, or a peer's route on a
                // node with no fleet left to ask. Not an error and not a silence to hide — the row
                // stays, keeps its reason, and stops being retried.
                _ => "this route no longer exists in the fleet",
            };
            store.delivery_abandoned(&pending.sink, pending.topic, pending.seq, now, why)?;
            continue;
        };

        // Re-derived from the log rather than stored, so there is one version of the fact
        // (ADR-0010: the event log stays the source of truth).
        let Some(note) = notification_at(store, pending.topic, pending.seq)? else {
            store.delivery_abandoned(
                &pending.sink,
                pending.topic,
                pending.seq,
                now,
                "its event is no longer in the log",
            )?;
            continue;
        };

        if let Some(reason) = &route.unusable {
            // Listed, and saying it cannot carry anything: waited on rather than given up on,
            // because the route is still there and the fleet still shows it. Nothing attempted,
            // so nothing spent — we never asked.
            store.delivery_deferred(
                &pending.sink,
                pending.topic,
                pending.seq,
                &format!("{} says {reason}; waiting for it", route.node_name),
            )?;
            continue;
        }

        if !route.reachable {
            // Nothing is attempted and nothing is spent. The row waits for the device to come
            // back, which is the overnight promise this plane exists to keep.
            store.delivery_deferred(
                &pending.sink,
                pending.topic,
                pending.seq,
                &format!("{} is not answering; waiting for it", route.node_name),
            )?;
            continue;
        }

        let sent = match (route.fires, route.node) {
            // A rule on this node: "delivering" it is firing it, and a refusal — a rule whose
            // occurrence is still going, a fleet that will not take the run — is retried like
            // any other undelivered notification, which is what the outbox is for.
            (Some(rule), _) => match fires {
                Some(fires) => fires.fire(rule, &note).await,
                None => Err("this node no longer fires rules".to_string()),
            },
            (None, None) => match sinks.get(&route.id) {
                Some(sink) => sink.deliver(&note).await,
                None => Err("this sink is no longer configured on this node".to_string()),
            },
            // The peer decides whether its own route still works; all this node chose was which
            // one to ask (ADR-0010: the credential never leaves the device that holds it).
            (None, Some(node)) => match fleet {
                Some(fleet) => match fleet.send(node, &route.id, &note).await {
                    Ok(()) => Ok(()),
                    Err(SendFailure::Refused(reason)) => Err(reason),
                    Err(SendFailure::Unanswered(reason)) => {
                        // Away, not gone: the same wait as a route the view already shows as
                        // not answering, reached a pass earlier. A node that has just restarted
                        // holds a stopped phone as alive until its detector says otherwise, and
                        // counting these gave up on its news in ten seconds (session ninety-four).
                        store.delivery_deferred(
                            &pending.sink,
                            pending.topic,
                            pending.seq,
                            &format!("{reason}; waiting for it"),
                        )?;
                        tracing::debug!(
                            about = %note.subject.topic(),
                            sink = %route.label(),
                            %reason,
                            "notification waiting for a device that did not answer"
                        );
                        continue;
                    }
                },
                None => Err("this node is no longer in a fleet".to_string()),
            },
        };

        match sent {
            Ok(()) => {
                store.delivery_sent(&pending.sink, pending.topic, pending.seq, now)?;
                delivered += 1;
                tracing::info!(
                    about = %note.subject.topic(),
                    sink = %route.label(),
                    notice = %note.title(),
                    "notification delivered"
                );
            }
            Err(reason) => {
                let attempts =
                    store.delivery_failed(&pending.sink, pending.topic, pending.seq, &reason)?;
                if attempts >= MAX_ATTEMPTS {
                    store.delivery_abandoned(
                        &pending.sink,
                        pending.topic,
                        pending.seq,
                        now,
                        &reason,
                    )?;
                    tracing::error!(
                        about = %note.subject.topic(),
                        sink = %route.label(),
                        attempts,
                        %reason,
                        "giving up on a notification; nobody has been told"
                    );
                } else {
                    tracing::warn!(
                        about = %note.subject.topic(),
                        sink = %route.label(),
                        attempts,
                        %reason,
                        "notification not delivered; will retry"
                    );
                }
            }
        }
    }
    Ok(delivered)
}

/// Every route this pass may use: this node's own, plus whatever the fleet advertises.
///
/// Two filters, and only one of them is here. A route whose holder says its credential does not
/// work is not a route at all and never reaches this list ([`Fleet::remote_routes`] drops it,
/// because a peer is believed about itself). A route whose holder is merely *asleep* is very much
/// a route: it is listed, its notifications are queued, and they wait — that is the difference
/// between a device that cannot help and one that is not looking yet.
///
/// Local routes are listed even when broken, because a missing script may be a script being
/// edited and it is this node's own claim either way.
fn all_routes(sinks: &Sinks, fleet: Option<&dyn Fleet>, fires: Option<&dyn Fires>) -> Vec<Route> {
    let mut routes: Vec<Route> = sinks
        .iter()
        .map(|sink| Route {
            node: None,
            id: sink.id().to_string(),
            service: sink.service().cloned(),
            node_name: "here".to_string(),
            reachable: true,
            // Our own broken route is attempted, fails, and is given up on loudly — see the
            // field's own note on why that is not the same answer as a peer's.
            unusable: None,
            fires: None,
        })
        .collect();
    if let Some(fleet) = fleet {
        routes.extend(fleet.remote_routes());
    }
    // …and this node's own notice-bound rules (ADR-0057 §2). A rule is a route under a key of
    // its own, so the cursor, the `(sink, topic, seq)` dedup, the ordering, the per-route queue
    // and the retries are the ones this plane already has — which is the entire reason the
    // escalation is expressed here rather than as a second scan over the event log.
    if let Some(fires) = fires {
        routes.extend(fires.rule_routes());
    }
    routes
}

/// Every route the rest of the fleet lists, and what is wrong with each one.
///
/// One function because there are two consumers with the same question and they used to answer
/// it differently: the delivery pass *filtered out* a peer whose credential does not verify,
/// while `offload sinks` listed it — so the pass called a route gone and the report showed it on
/// the next line. Listed-and-unusable is a state, not an absence ([`Route::unusable`]).
///
/// Peers only: this node's own routes come from its config, and it is entitled to a different
/// answer about them.
#[must_use]
pub fn routes_in(view: &offload_core::ClusterView) -> Vec<Route> {
    peer_routes(view).map(|(route, _)| route).collect()
}

/// The one walk over a view's peer sinks, with the capability each route came from.
///
/// Private, and the reason the two public callers cannot drift: the pass wants the route and the
/// report wants the route *and* what the capability says about itself.
fn peer_routes(
    view: &offload_core::ClusterView,
) -> impl Iterator<Item = (Route, &offload_core::Capability)> {
    view.nodes
        .values()
        .filter(|node| node.id != view.local)
        .flat_map(|node| {
            node.capabilities
                .playing(offload_core::Role::Sink)
                .map(move |capability| {
                    let route = Route {
                        node: Some(node.id),
                        id: sink_id_of(&capability.id.0).to_string(),
                        // A peer's own declaration of what its route is, which is what a run's
                        // audience is matched against. The command behind it stays where it is.
                        service: Some(capability.service.clone()),
                        node_name: node.display_name(),
                        // Every node this one knows, not only the ones answering: a route on a
                        // sleeping device is still a route, and its news waits for it.
                        reachable: node.status.is_available(),
                        unusable: offload_core::deliverability(capability)
                            .err()
                            .map(|why| why.to_string()),
                        fires: None,
                    };
                    (route, capability)
                })
        })
}

/// Rebuild the notification an outbox row refers to, from the log.
fn notification_at(
    store: &offload_store::Store,
    topic: Topic,
    seq: u64,
) -> Result<Option<Notification>, offload_store::StoreError> {
    match topic {
        Topic::Run => {
            let Some((run, json)) = store.event_at(seq)? else {
                return Ok(None);
            };
            let Ok(event) = serde_json::from_str::<LogEvent>(&json) else {
                tracing::error!(seq, "a notable event no longer decodes; not delivering it");
                return Ok(None);
            };
            Ok(notify::notable(run, seq, &event))
        }
        Topic::Fleet => {
            let Some(json) = store.fleet_event_at(seq)? else {
                return Ok(None);
            };
            let Ok(event) = serde_json::from_str::<offload_core::FleetLogEvent>(&json) else {
                tracing::error!(seq, "a fleet event no longer decodes; not delivering it");
                return Ok(None);
            };
            Ok(notify::notable_fleet(seq, &event))
        }
    }
}

/// What `offload sinks` shows: what is configured, whether it works, and what it has done.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SinkReport {
    pub id: String,
    pub service: String,
    pub description: String,
    /// How it is invoked. **This node's own view only** — it is absent from [`FleetRoute`] on
    /// purpose, because a peer learns that this device has a route and never how it works.
    #[serde(default)]
    pub command: String,
    /// `None` when it is usable; the reason when it is not.
    pub unusable: Option<String>,
    pub delivered: u64,
    pub waiting: u64,
    /// Retried to exhaustion and abandoned. Its own number, because a route that has never
    /// worked must not report a healthy count.
    pub gave_up: u64,
    pub last_error: Option<String>,
}

/// A route on another node, as this one has heard about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FleetRoute {
    pub node: String,
    pub id: String,
    pub service: String,
    pub description: String,
    /// Its holder says the credential works. `false` is why a route is *not* used, and it is a
    /// fact about that device rather than a failure here — so it is shown rather than hidden.
    pub authenticated: bool,
    /// Whether the holder is answering right now. A route on a node that has gone quiet is not
    /// used — asking costs a timeout per pass — but it is *shown*, because "the only route in
    /// this fleet is on a device that is asleep" is the explanation for a silence, and omitting
    /// the row makes the answer "there is no such route".
    pub reachable: bool,
    pub delivered: u64,
    pub waiting: u64,
    pub gave_up: u64,
    pub last_error: Option<String>,
}

/// Describe every configured route, including the broken ones.
///
/// Including the broken ones is the point: a sink that cannot be used is precisely what
/// somebody is looking for when they ask why nothing arrived, and leaving it out of the list
/// would make the answer "there is no such sink".
pub fn report(config: &Config, store: &offload_store::Store) -> Vec<SinkReport> {
    let mut out = Vec::new();
    for cfg in config.sinks.iter().filter(|cfg| !cfg.id.is_empty()) {
        let totals = store.sink_totals(&cfg.id).unwrap_or_default();
        out.push(SinkReport {
            id: cfg.id.clone(),
            service: cfg.service.clone(),
            description: cfg.description.clone(),
            command: cfg.command.clone(),
            unusable: ExecSink::new(cfg).usable().err(),
            delivered: totals.delivered,
            waiting: totals.waiting,
            gave_up: totals.gave_up,
            last_error: totals.last_error,
        });
    }
    out
}

/// The notification `offload sinks --test` sends: real enough to prove the route, obviously
/// not real news.
///
/// Deliberately shaped like the thing it is testing rather than a "hello world": a script that
/// works on a synthetic string and then trips over a real summary has not been tested. The run
/// id is all zeroes, which is not a run and reads as one, so anything the script logs is
/// identifiable afterwards.
///
/// `Failed` rather than `Finished`, and that is a choice rather than the first variant to hand.
/// Every `Notice` is an event about a run and none of them is "this is a test", so whichever is
/// used says something untrue on somebody's phone — and of the two, "something went wrong" is the
/// safe lie. A test that announced a run had *finished* is the one a person might believe, and
/// believing it means not going to look. The text says what it is either way; the kind is what
/// survives being read at a glance. Not worth a `Notice::Test` variant: the notification kinds are
/// the small typed subset of events worth interrupting somebody for (ADR-0010), a diagnostic is
/// not one, and every sink script in every fleet would have to learn it.
#[must_use]
pub fn probe_notice() -> Notification {
    Notification {
        subject: offload_core::Subject::run(offload_core::RunId::from_bytes([0; 16])),
        seq: 0,
        at: Millis(0),
        notice: offload_core::Notice::Failed {
            reason: "this is a test from `offload sinks --test`, not a real run".into(),
        },
    }
}

/// What the rest of the fleet offers, and what this node has managed to send through it.
///
/// Both halves matter and they come from different places: the *route* is a gossiped capability,
/// and the *counts* are this node's own outbox — because the node that logged an event is the one
/// that remembers whether anybody was told. A peer showing zero deliveries here has not failed;
/// it has simply never been asked by this machine.
///
/// Routes that cannot be used right now are included — unauthenticated ones, and ones on a node
/// that has gone quiet. They are the answer to "there is a phone in this fleet, why is nothing
/// reaching it", and omitting them would make the answer "there is no phone". The delivery pass
/// attempts neither and gives up on neither: both are listed, so their news waits (see
/// [`routes_in`], which is where the pass gets the same two facts).
#[must_use]
pub fn fleet_routes(
    view: &offload_core::ClusterView,
    store: &offload_store::Store,
) -> Vec<FleetRoute> {
    peer_routes(view)
        .map(|(route, capability)| {
            let totals = store.sink_totals(&route.key()).unwrap_or_default();
            FleetRoute {
                node: route.node_name.clone(),
                id: route.id.clone(),
                service: capability.service.to_string(),
                description: capability.description.clone(),
                authenticated: capability.authenticated,
                reachable: route.reachable,
                delivered: totals.delivered,
                waiting: totals.waiting,
                gave_up: totals.gave_up,
                last_error: totals.last_error,
            }
        })
        .collect()
}

/// What to tell somebody, at the moment they submit, about where their run's news will go.
///
/// `None` for the default audience: a sentence printed on every submission is one nobody reads on
/// the submission where it matters.
///
/// Answered *here*, while the person is still at the keyboard, for ADR-0014's reason. A run that
/// asked for a route no device in this fleet offers will finish in the night and tell nobody, and
/// the difference between learning that now and learning it at 08:00 is the whole argument of that
/// ADR applied to the other plane. It is a **note and never a refusal** — the work is still worth
/// doing, and this project does not cancel runs over reporting.
///
/// Both halves of the fleet are asked, because both can carry it: this node's own configured
/// routes, and the ones peers advertise. A local route that is broken still counts as a route —
/// its news is queued and retried — but it is called out, because "the only push route here is a
/// script that has been deleted" is a different answer from "there is no push route".
#[must_use]
pub fn audience_note(
    audience: &offload_core::Audience,
    config: &Config,
    view: Option<&offload_core::ClusterView>,
) -> Option<String> {
    let wanted = match audience {
        offload_core::Audience::Everyone => return None,
        // Said back, for the deadline's reason: it changes behaviour and there is no later event
        // to point at. A silence somebody chose is not a silence this plane has to explain.
        offload_core::Audience::Nobody => {
            return Some("nobody will be told when it finishes — you asked for none".to_string())
        }
        offload_core::Audience::Service { service } => service,
    };

    let mut usable = Vec::new();
    let mut broken = Vec::new();
    for cfg in config.sinks.iter().filter(|cfg| !cfg.id.is_empty()) {
        if cfg.service().ok().as_ref() != Some(wanted) {
            continue;
        }
        match ExecSink::new(cfg).usable() {
            Ok(()) => usable.push(cfg.id.clone()),
            Err(_) => broken.push(cfg.id.clone()),
        }
    }
    // A peer's routes carry this run's problems and never that it finished (ADR-0073): the
    // submission is here, so this node's own routes are the only ones good news goes to.
    let local = usable.clone();
    for route in view.map(fleet_sinks).unwrap_or_default() {
        if &route.1 == wanted {
            usable.push(route.0);
        }
    }

    if usable.len() > local.len() {
        let finished = if local.is_empty() {
            format!("no device: this one has no {wanted} route")
        } else {
            local.join(", ")
        };
        return Some(format!(
            "{wanted} goes to {} — that it finished, only to {finished}",
            usable.join(", ")
        ));
    }
    if !usable.is_empty() {
        return Some(format!("{wanted} goes to {}", usable.join(", ")));
    }
    if !broken.is_empty() {
        // The route exists and cannot be used, which is the answer with a fix attached.
        return Some(format!(
            "the only {wanted} route here ({}) cannot be used — `offload sinks` says why",
            broken.join(", ")
        ));
    }
    Some(format!(
        "no {wanted} route in this fleet, so nothing will tell you — `offload sinks` lists what \
         there is"
    ))
}

/// What `--ask` amounts to here, when that is not what it says on the tin.
///
/// A function of its own for the reason `log_source` is one: the path that decides it is a
/// submission over a live socket, and the only test that could have caught its absence is one
/// that can call this. `None` is the ordinary answer twice over — the run did not ask, or there
/// is somebody to ask.
#[must_use]
pub fn ask_note(ask: offload_core::AskPolicy, can_reach: bool) -> Option<String> {
    if matches!(ask, offload_core::AskPolicy::Never) || can_reach {
        return None;
    }
    Some(
        "nothing in this fleet can reach a person, so --ask will stop nothing — its questions \
         are decided by the agent's own rules, as they are without it"
            .to_string(),
    )
}

/// Whether a rule's news has anywhere to go — and if not, which of the two silences it is.
///
/// A bool would collapse a distinction this plane keeps paying for: a silence somebody **asked**
/// for is not a silence the fleet imposed. `--notify nobody` is a decision, and printing "add a
/// route" under it would be advice about a setting the author chose on purpose; no route at all is
/// a gap with a fix attached. Same shape as `Removal::{Removed, NothingHere}` — one word for two
/// facts is how a report comes to answer the wrong one.
///
/// Asked by `offload when` and not by `offload run`, because the two differ in exactly one place
/// and it is the **default**. `audience_note` returns `None` for `Audience::Everyone` on the
/// stated grounds that it "needs no words", which is true in front of somebody at a keyboard and
/// false for a watcher that will fire unattended for months with nothing in the fleet able to
/// carry what it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    /// Some route in this fleet carries it.
    Somebody,
    /// The author asked for none.
    NobodyWanted,
    /// Nothing in this fleet can carry it.
    NoRoute,
}

/// Where a rule's news can get to, on this fleet, right now.
#[must_use]
pub fn reach(
    audience: &offload_core::Audience,
    config: &Config,
    view: Option<&offload_core::ClusterView>,
) -> Reach {
    if matches!(audience, offload_core::Audience::Nobody) {
        return Reach::NobodyWanted;
    }
    let local = config
        .sinks
        .iter()
        .filter(|cfg| !cfg.id.trim().is_empty())
        .filter_map(|cfg| cfg.service().ok());
    let peers = view
        .map(fleet_sinks)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, service)| service);
    // Through `Audience::admits`, per route, because that is exactly what the delivery pass asks
    // (ADR-0010: an audience selects routes, so the question is asked per capability). A second
    // copy of the matching rule here is how a report comes to disagree with the plane it
    // describes. A configured route that is broken or asleep counts, for `can_reach_a_person`'s
    // reason: it is a route, and `audience_note` is what says it cannot be used today.
    if local
        .chain(peers)
        .any(|service| audience.admits(Some(&service)))
    {
        Reach::Somebody
    } else {
        Reach::NoRoute
    }
}

/// Is there anybody in this fleet a question could reach (ADR-0017)?
///
/// One predicate, because it decides two things that must not disagree: whether a blocked agent's
/// question is *put* to anybody, and whether `offload run --ask` says so while the operator is
/// still at the keyboard. It used to be spelled out at the first of those alone, so a run
/// submitted with `--ask` to a fleet with no route was accepted without comment and then behaved
/// exactly like a run without it — the promise the flag makes is the one thing nothing said could
/// not be kept.
///
/// A configured sink counts whether or not it currently works, for `fleet_routes`' reason: a route
/// that is broken or asleep is a route, and "there is nobody to ask" is a different sentence from
/// "the phone is locked".
#[must_use]
pub fn can_reach_a_person(
    config: &Config,
    view: Option<&offload_core::ClusterView>,
    store: &offload_store::Store,
) -> bool {
    !config.sinks.iter().all(|cfg| cfg.id.trim().is_empty())
        || view.is_some_and(|view| !fleet_routes(view, store).is_empty())
}

/// Every usable route the fleet advertises, as `(label, service)`.
///
/// Peers only, and only the ones whose holder says the credential works: a peer is believed about
/// itself, which is the same rule the delivery pass follows. Unreachable devices are *included* —
/// a phone that is asleep is a route that will be used when it wakes up, and telling somebody
/// their news has nowhere to go because their phone is locked would be a lie.
fn fleet_sinks(view: &offload_core::ClusterView) -> Vec<(String, offload_core::Service)> {
    let mut out = Vec::new();
    for node in view.nodes.values().filter(|node| node.id != view.local) {
        for capability in node.capabilities.playing(offload_core::Role::Sink) {
            if offload_core::deliverability(capability).is_err() {
                continue;
            }
            out.push((
                format!("{}/{}", node.display_name(), sink_id_of(&capability.id.0)),
                capability.service.clone(),
            ));
        }
    }
    out
}

/// How far the delivery plane has scanned the run log, across every route that exists.
///
/// The watermark ADR-0021 §3 prunes behind: an event above it is one some route has not been
/// shown yet, and deleting it would destroy the news *before* the outbox row promising to deliver
/// it is ever written. That is the half of "still owed" with nothing to point at — the outbox
/// answers the other half, and asking only it is the trap.
///
/// Two rules, and both are about which routes count:
///
/// * **The ones that exist right now**, which is the same set the pass itself walks
///   ([`all_routes`]). A cursor left behind by a peer that has gone never advances again, so
///   reading it would pin every record on this node for ever.
/// * **A route with no cursor row does not hold anything back.** [`Store::sink_cursor`]
///   initialises a new one to the *end* of the log, precisely so that configuring a route today
///   does not replay a month at somebody — which means a route that has never scanned will never
///   want these events. `u64::MAX` is the honest answer for it, not zero.
///
/// No routes at all is `u64::MAX` for the same reason: nothing can ever be owed.
#[must_use]
pub fn scanned_to(
    config: &Config,
    store: &offload_store::Store,
    view: Option<&offload_core::ClusterView>,
) -> u64 {
    let local = config
        .sinks
        .iter()
        .filter(|cfg| !cfg.id.is_empty())
        .map(|cfg| cfg.id.clone());
    let remote: Vec<String> = view
        .map(|view| peer_routes(view).map(|(route, _)| route.key()).collect())
        .unwrap_or_default();

    local
        .chain(remote)
        .map(|key| {
            store
                .existing_sink_cursor(&key, Topic::Run)
                .ok()
                .flatten()
                .unwrap_or(u64::MAX)
        })
        .min()
        .unwrap_or(u64::MAX)
}

/// Every sink's outbox position, so `offload sinks` can say where a route has reached.
#[must_use]
pub fn cursors(config: &Config, store: &offload_store::Store) -> BTreeMap<String, u64> {
    config
        .sinks
        .iter()
        .filter(|cfg| !cfg.id.is_empty())
        .filter_map(|cfg| {
            // Read-only: `sink_cursor` would *create* one, and a status command must not make
            // a sink start reporting from the moment somebody looked at it.
            store
                // The run log's position. The fleet log has its own cursor and is a handful of
                // entries in a fleet's life, so a route's "where has it reached" is about runs.
                .existing_sink_cursor(&cfg.id, Topic::Run)
                .ok()
                .flatten()
                .map(|seq| (cfg.id.clone(), seq))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offload_core::{AgentWork, Work};
    use offload_core::{LogKind, Notice, RunId};

    /// ADR-0080: a model list is claimed only for the agent it was read from. A list read from an
    /// older version, or while logged in when the agent is now logged out, is not this agent's
    /// list, so nothing is advertised until it is read again.
    #[test]
    fn a_model_list_is_advertised_only_for_the_agent_version_and_login_it_was_read_from() {
        let agent = |version: &str, authenticated: bool| {
            let mut caps = offload_core::Capabilities::empty(
                offload_core::Os::Linux,
                offload_core::Arch::X86_64,
                offload_core::DeviceClass::Desktop,
            );
            caps.add(offload_core::Capability::agent(
                offload_core::AgentKind::ClaudeCode,
                offload_core::AgentDetails {
                    version: version.into(),
                    models: Vec::new(),
                    max_concurrent: 2,
                },
                authenticated,
            ));
            caps
        };
        let listed = |caps: &offload_core::Capabilities| -> Vec<String> {
            caps.agent_details(&offload_core::AgentKind::ClaudeCode)
                .map(|d| d.models.iter().map(|m| m.value.clone()).collect())
                .unwrap_or_default()
        };
        let models = Models::default();
        assert!(models.store(ModelsRead {
            version: "2.1.283".into(),
            authenticated: true,
            models: vec![
                offload_core::Model::named("opus"),
                offload_core::Model::named("haiku")
            ],
        }));
        assert!(
            !models.store(models.held().expect("held")),
            "the same reading is no change"
        );

        let mut same = agent("2.1.283", true);
        with_models(&mut same, &models);
        assert_eq!(listed(&same), ["opus", "haiku"]);

        let mut updated = agent("2.1.290", true);
        with_models(&mut updated, &models);
        assert!(listed(&updated).is_empty(), "read from another version");

        let mut logged_out = agent("2.1.283", false);
        with_models(&mut logged_out, &models);
        assert!(
            listed(&logged_out).is_empty(),
            "a logged-out agent reaches nothing"
        );

        let mut none = offload_core::Capabilities::empty(
            offload_core::Os::Linux,
            offload_core::Arch::X86_64,
            offload_core::DeviceClass::Desktop,
        );
        with_models(&mut none, &models);
        assert!(
            none.agent(&offload_core::AgentKind::ClaudeCode).is_none(),
            "no agent conjured"
        );
    }

    /// ADR-0066 §3: the host's word on power and metered counts while it is fresh and not after,
    /// and never over the owner's own `metered` nomination.
    #[test]
    fn host_facts_count_while_fresh_and_never_over_a_nomination() {
        let dir = std::env::temp_dir().join(format!("offload-hostfacts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("host-facts.json"),
            r#"{"battery_percent":87,"charging":true,"metered":true,"form":"tablet"}"#,
        )
        .expect("write");
        let now = std::time::SystemTime::now();

        let facts = host_facts(&dir, now).expect("a fresh file is read");
        let mut caps = offload_core::Capabilities::empty(
            offload_core::Os::Android,
            offload_core::Arch::Aarch64,
            offload_core::DeviceClass::Phone,
        );
        facts.apply(&mut caps, false);
        assert_eq!(
            caps.power,
            offload_core::PowerSource::Battery {
                percent: 87,
                charging: true
            }
        );
        assert_eq!(caps.metered_network, offload_core::Metered::Yes);
        assert_eq!(
            caps.device_class,
            offload_core::DeviceClass::Tablet,
            "the host's word on form"
        );

        // The owner said the link is free: that stands, whatever the host thinks.
        let mut nominated = offload_core::Capabilities::empty(
            offload_core::Os::Android,
            offload_core::Arch::Aarch64,
            offload_core::DeviceClass::Phone,
        );
        nominated.metered_network = offload_core::Metered::No;
        facts.apply(&mut nominated, true);
        assert_eq!(nominated.metered_network, offload_core::Metered::No);

        // Ten minutes on, the host has stopped telling, and what it said last is not believed.
        let later = now + std::time::Duration::from_secs(600);
        assert_eq!(host_facts(&dir, later), None);
        // …and a file that is not the shape is no answer, not a zero.
        std::fs::write(dir.join("host-facts.json"), "battery: lots").expect("write");
        assert_eq!(host_facts(&dir, now), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn note() -> Notification {
        Notification {
            subject: offload_core::Subject::run(RunId::from_bytes([4; 16])),
            seq: 7,
            at: Millis(1),
            notice: Notice::Failed {
                reason: "the agent stopped".into(),
            },
        }
    }

    fn config_with(sinks: Vec<SinkConfig>) -> Config {
        Config {
            sinks,
            ..Config::default()
        }
    }

    fn sink(id: &str, command: &str) -> SinkConfig {
        SinkConfig {
            id: id.into(),
            service: "push".into(),
            command: command.into(),
            ..SinkConfig::default()
        }
    }

    #[test]
    fn the_node_advertises_the_agent_it_would_spawn() {
        // The bug this replaced: the daemon probed `claude` on `PATH` and spawned
        // `agent.binary`, so an owner who said where their agent is got a node advertising a
        // different program's version and authentication — or, with no `claude` on `PATH`,
        // advertising no agent at all while its configured one ran perfectly. Measured on two
        // daemons before it was fixed: `offload status` said `none installed` on a machine with
        // an authenticated install one directory away.
        //
        // The script is deliberately not named `claude`, so a version that came from `PATH`
        // fails this rather than matching by luck.
        let dir = std::env::temp_dir().join(format!("offload-agentbin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let binary = dir.join("my-agent");
        std::fs::write(&binary, "#!/bin/sh\necho '9.9.9 (Claude Code)'\n").expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut config = Config::default();
        config.agent.binary = binary;
        let caps = capabilities(&config, &Models::default());
        assert_eq!(
            caps.agent_details(&offload_core::AgentKind::ClaudeCode)
                .map(|d| d.version.as_str()),
            Some("9.9.9")
        );

        // And the other direction, which is the one that decides whether this node bids at all.
        let mut missing = Config::default();
        missing.agent.binary = "/definitely/not/here/claude".into();
        assert!(capabilities(&missing, &Models::default())
            .agent(&offload_core::AgentKind::ClaudeCode)
            .is_none());
    }

    #[test]
    fn asking_a_fleet_that_can_reach_nobody_is_said_at_the_keyboard() {
        // `--ask` was accepted without comment on a fleet with no route, and the run then behaved
        // exactly as if it had not been passed: the question is never put, the agent's own rules
        // decide, and the only trace was a `debug` line on the holder hours later. Measured on a
        // daemon with no sinks — the flag changes what the *run* does, so it is the one that must
        // not fail quietly (ADR-0014).
        let asking = offload_core::AskPolicy::UpTo { questions: 3 };
        assert!(ask_note(asking, false).is_some());
        assert_eq!(ask_note(asking, true), None, "there is somebody to ask");
        assert_eq!(
            ask_note(offload_core::AskPolicy::Never, false),
            None,
            "a run that never asks is not owed a note about asking"
        );

        // And the predicate the submission and the blocked agent both read, so they cannot say
        // different things about one fleet. A route counts before it is known to work: broken or
        // asleep is a route, and "nobody to ask" is a different sentence.
        let store = offload_store::Store::open_memory().expect("store");
        assert!(!can_reach_a_person(&config_with(vec![]), None, &store));
        assert!(!can_reach_a_person(
            &config_with(vec![sink("", "true")]),
            None,
            &store
        ));
        assert!(can_reach_a_person(
            &config_with(vec![sink("phone", "/definitely/not/here")]),
            None,
            &store
        ));
    }

    /// The three answers a rule's audience can have, and why a bool would have been two of them.
    ///
    /// `offload when` printed "you will hear about a failure, a missed deadline or a question" on a
    /// node with no route to a person — three notifications, none of them possible — and did so
    /// for the *plainest* invocation of the command, because the default audience is exactly the
    /// case `audience_note` is deliberately silent about. So this is asked with no flags as well
    /// as with them.
    #[test]
    fn a_rules_news_has_three_places_it_can_get_to_and_they_are_different_sentences() {
        let default_audience = offload_core::Audience::Everyone;
        let asked_for_none = offload_core::Audience::Nobody;
        let by_service = |s: &str| offload_core::Audience::Service {
            service: s.parse().expect("service"),
        };

        // The case that was wrong and that nothing said: no flags at all, and nowhere to go.
        assert_eq!(
            reach(&default_audience, &config_with(vec![]), None),
            Reach::NoRoute
        );
        // A silence the author asked for is not the fleet's fault, and must not come with advice
        // about adding a route — the whole reason this is not a bool.
        assert_eq!(
            reach(
                &asked_for_none,
                &config_with(vec![sink("phone", "true")]),
                None
            ),
            Reach::NobodyWanted,
            "a route exists; the author still said none"
        );
        assert_eq!(
            reach(&asked_for_none, &config_with(vec![]), None),
            Reach::NobodyWanted,
            "and the choice is reported ahead of the gap, because it is the reason"
        );
        // A configured route counts before it is known to work, for `can_reach_a_person`'s reason:
        // broken or asleep is a route, and `audience_note` is what says it cannot be used today.
        assert_eq!(
            reach(
                &default_audience,
                &config_with(vec![sink("phone", "/definitely/not/here")]),
                None
            ),
            Reach::Somebody
        );
        // Named by service, matched through `Audience::admits` so this cannot drift from what the
        // delivery pass actually asks per route.
        assert_eq!(
            reach(
                &by_service("push"),
                &config_with(vec![sink("phone", "true")]),
                None
            ),
            Reach::Somebody
        );
        assert_eq!(
            reach(
                &by_service("email"),
                &config_with(vec![sink("phone", "true")]),
                None
            ),
            Reach::NoRoute,
            "one route, and it is not the one asked for"
        );
        // An id-less entry is not a route, the same way it is not one for `can_reach_a_person`.
        assert_eq!(
            reach(
                &default_audience,
                &config_with(vec![sink("", "true")]),
                None
            ),
            Reach::NoRoute
        );
    }

    #[test]
    fn the_owner_can_raise_what_the_probe_guessed_the_install_sustains() {
        // The probe's 2 is a cap that binds before the owner's own on every desktop, server and
        // VM, where `max_concurrent_runs` defaults to 4 — so the number they *could* set did
        // nothing above 2 and the refusal named a "per-node concurrency limit" no config held.
        let dir = std::env::temp_dir().join(format!("offload-agentcap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let binary = dir.join("my-agent");
        std::fs::write(&binary, "#!/bin/sh\necho '9.9.9 (Claude Code)'\n").expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut config = Config::default();
        config.agent.binary = binary;
        assert_eq!(
            capabilities(&config, &Models::default())
                .agent_details(&offload_core::AgentKind::ClaudeCode)
                .map(|d| d.max_concurrent),
            Some(2),
            "unset is the probe's conservative guess"
        );

        config.agent.max_concurrent = Some(crate::config::AgentConcurrency(6));
        assert_eq!(
            capabilities(&config, &Models::default())
                .agent_details(&offload_core::AgentKind::ClaudeCode)
                .map(|d| d.max_concurrent),
            Some(6)
        );

        // And it describes an agent rather than conjuring one: a number beside a binary that is
        // not there is the over-claim the probe refuses, arriving through the config instead.
        let mut absent = Config::default();
        absent.agent.binary = "/definitely/not/here/claude".into();
        absent.agent.max_concurrent = Some(crate::config::AgentConcurrency(6));
        assert!(capabilities(&absent, &Models::default())
            .agent(&offload_core::AgentKind::ClaudeCode)
            .is_none());
    }

    #[test]
    fn a_node_configured_for_one_account_refuses_to_claim_another() {
        // ADR-0028. `agent.config_dir` selects the account and `agent.account` says which one the
        // owner meant, so the pair is "this device may run agents on my work login and no other".
        // The wrong answer here spends the wrong account's money and nothing else would notice.
        let dir = std::env::temp_dir().join(format!("offload-acct-{}", std::process::id()));
        let state = dir.join("state");
        std::fs::create_dir_all(&state).expect("scratch");
        let binary = dir.join("my-agent");
        std::fs::write(
            &binary,
            "#!/bin/sh
echo '9.9.9 (Claude Code)'
",
        )
        .expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        // What a logged-in agent's state directory looks like: a credential, and the uuid the
        // fingerprint is derived from.
        std::fs::write(state.join(".credentials.json"), "{}").expect("write");
        std::fs::write(
            state.join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"11111111-1111-1111-1111-111111111111"}}"#,
        )
        .expect("write");
        let account =
            offload_core::capability::AccountId::of_account("11111111-1111-1111-1111-111111111111");

        let mut config = Config::default();
        config.agent.binary = binary;
        config.agent.config_dir = Some(state.clone());

        // The nominated directory is where the answer came from, so the node is authenticated
        // *as that account* — which is the whole reason the path has to reach the probe.
        let caps = capabilities(&config, &Models::default());
        let agent = caps
            .agent(&offload_core::AgentKind::ClaudeCode)
            .expect("an agent");
        assert!(agent.authenticated);
        assert_eq!(agent.identity.as_ref(), Some(&account));

        // Named correctly: nothing changes.
        config.agent.account = Some(account.0.clone());
        let agent = capabilities(&config, &Models::default())
            .agent(&offload_core::AgentKind::ClaudeCode)
            .cloned()
            .expect("an agent");
        assert!(agent.authenticated);

        // Named as somebody else: still installed, and no longer usable — reported as *not
        // authenticated*, which is the state that already means "refuses every run and says
        // why", rather than as a missing agent, which would send somebody looking for a program
        // that is right there.
        config.agent.account = Some("acct:somebodyelse".to_string());
        let agent = capabilities(&config, &Models::default())
            .agent(&offload_core::AgentKind::ClaudeCode)
            .cloned()
            .expect("still installed");
        assert!(!agent.authenticated);
        assert!(
            agent.description.contains(&account.0) && agent.description.contains("somebodyelse"),
            "the refusal names both, or it is unactionable: {}",
            agent.description
        );

        // And a guard beside no agent at all conjures nothing — `set_agent_concurrency`'s rule,
        // for the same reason.
        let mut absent = Config::default();
        absent.agent.binary = "/definitely/not/here/claude".into();
        absent.agent.account = Some(account.0.clone());
        assert!(capabilities(&absent, &Models::default())
            .agent(&offload_core::AgentKind::ClaudeCode)
            .is_none());
    }

    #[test]
    fn a_route_whose_program_is_missing_is_not_authenticated() {
        // ADR-0010's sharpest rule, and the only part of a delivery route this node can
        // actually verify. A sink that claimed to work would win the routing decision and then
        // drop the message, which is worse than not offering it at all.
        let caps = sink_capabilities(&config_with(vec![
            sink("real", "true"),
            sink("imaginary", "/definitely/not/here"),
        ]));
        assert_eq!(caps.len(), 2);
        assert!(caps[0].authenticated, "`true` is on every PATH");
        assert!(!caps[1].authenticated);
        assert_eq!(notify::deliverability(&caps[0]), Ok(()));
        assert_eq!(
            notify::deliverability(&caps[1]),
            Err(notify::Undeliverable::Unauthenticated)
        );
    }

    #[test]
    fn the_service_is_the_owners_declaration_and_the_command_is_not() {
        // What makes "reach me by push" expressible later: every exec sink would otherwise be
        // indistinguishable from every other one.
        let mut email = sink("home", "true");
        email.service = "email".into();
        email.identity = Some("me@example.com".into());
        let caps = sink_capabilities(&config_with(vec![email]));
        assert_eq!(caps[0].service, offload_core::Service::Email);
        assert_eq!(caps[0].id.0, "sink:home");
        assert_eq!(
            caps[0].identity.as_ref().map(|a| a.0.as_str()),
            Some("me@example.com")
        );
        assert!(caps[0].plays(offload_core::Role::Sink));
    }

    #[test]
    fn a_sink_with_no_id_is_dropped_rather_than_advertised_blank() {
        // The id is a dedup key, so a blank one collides with the next blank one — which would
        // silently mean two routes sharing an outbox.
        let caps = sink_capabilities(&config_with(vec![sink("", "true")]));
        assert!(caps.is_empty());
        assert!(Sinks::from_config(&config_with(vec![sink("", "true")])).is_empty());
    }

    #[tokio::test]
    async fn a_delivery_carries_the_notification_three_ways() {
        // JSON on stdin, `OFFLOAD_*` in the environment, and the summary as a final argument:
        // enough that a two-line script can be written in whatever style its author likes,
        // with no template language to get the quoting wrong in.
        let script = std::env::temp_dir().join(format!("offload-sink-{}.sh", std::process::id()));
        let out = std::env::temp_dir().join(format!("offload-sink-{}.out", std::process::id()));
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n{{ echo \"arg=$1\"; echo \"env=$OFFLOAD_TITLE\"; cat; }} > {}\n",
                out.display()
            ),
        )
        .expect("write script");
        let mut perms = std::fs::metadata(&script).expect("stat").permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        std::fs::set_permissions(&script, perms).expect("chmod");

        let exec = ExecSink::new(&sink("t", &script.display().to_string()));
        exec.deliver(&note()).await.expect("delivered");

        let written = std::fs::read_to_string(&out).expect("output");
        assert!(
            written.contains("arg=failed: the agent stopped"),
            "{written}"
        );
        assert!(written.contains("env=run 040404040404 failed"), "{written}");
        // Readable rather than a byte array: this JSON is an interface for somebody's script.
        assert!(
            written.contains("\"run\":\"04040404040404040404040404040404\""),
            "{written}"
        );
        assert!(written.contains("\"kind\":\"failed\""), "{written}");

        let _ = std::fs::remove_file(&script);
        let _ = std::fs::remove_file(&out);
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_own_words() {
        // "exit 1" tells nobody whether the token was wrong or the network was down.
        let exec = ExecSink::new(&sink("t", "sh"));
        let mut with_args = sink("t", "sh");
        with_args.args = vec!["-c".into(), "echo no such topic >&2; exit 3".into()];
        assert!(exec.usable().is_ok());

        let failing = ExecSink::new(&with_args);
        let error = failing.deliver(&note()).await.expect_err("should fail");
        assert!(error.contains("exited 3"), "{error}");
        assert!(error.contains("no such topic"), "{error}");
    }

    #[tokio::test]
    async fn a_missing_command_fails_the_delivery_rather_than_the_pass() {
        let exec = ExecSink::new(&sink("t", "/definitely/not/here"));
        let error = exec.deliver(&note()).await.expect_err("should fail");
        assert!(error.contains("not found on this device"), "{error}");
    }

    /// A sink that counts what it was given and can be told to fail.
    #[derive(Debug)]
    struct Counting {
        id: String,
        fail: bool,
        service: Option<offload_core::Service>,
        seen: std::sync::Mutex<Vec<u64>>,
    }

    #[async_trait::async_trait]
    impl Sink for Counting {
        fn id(&self) -> &str {
            &self.id
        }
        fn usable(&self) -> Result<(), String> {
            Ok(())
        }
        fn service(&self) -> Option<&offload_core::Service> {
            self.service.as_ref()
        }
        async fn deliver(&self, note: &Notification) -> Result<(), String> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(note.seq);
            if self.fail {
                return Err("the phone is off".into());
            }
            Ok(())
        }
    }

    fn seen(sink: &Arc<Counting>) -> std::sync::MutexGuard<'_, Vec<u64>> {
        sink.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn counting(id: &str, fail: bool) -> Arc<Counting> {
        Arc::new(Counting {
            id: id.into(),
            fail,
            service: Some(offload_core::Service::Push),
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The same, offering a service somebody might not have asked for.
    fn counting_as(id: &str, service: offload_core::Service) -> Arc<Counting> {
        Arc::new(Counting {
            id: id.into(),
            fail: false,
            service: Some(service),
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn store_with_a_run() -> (offload_store::Store, RunId) {
        store_with_run_for(offload_core::Audience::Everyone)
    }

    fn store_with_run_for(notify: offload_core::Audience) -> (offload_store::Store, RunId) {
        store_with_run_wanting(notify, offload_core::Notices::Everything)
    }

    fn store_with_run_wanting(
        notify: offload_core::Audience,
        notices: offload_core::Notices,
    ) -> (offload_store::Store, RunId) {
        let store = offload_store::Store::open_memory().expect("store");
        let id = RunId::from_bytes([9; 16]);
        let spec = offload_core::RunSpec {
            work: Work::Agent(AgentWork {
                agent: offload_core::AgentKind::ClaudeCode,
                model: None,
                prompt: "p".into(),
                workspace: offload_core::WorkspaceSpec {
                    repo: "/repo".into(),
                    archive_bytes: None,
                    git_ref: None,
                    branch: None,
                },
                permission_mode: offload_core::PermissionMode::AcceptEdits,
                allow: offload_core::ToolAllowlist::default(),
                max_turns: None,
                ask: offload_core::AskPolicy::Never,
            }),
            constraint: offload_core::Constraint::Always,
            restartability: offload_core::Restartability::Resumable,
            priority: 0,
            queue: false,
            deadline: None,
            demand: offload_core::Demand::Normal,
            notify,
            notices,
            resources: Vec::new(),
            prefer: offload_core::Constraint::Always,
            hold_until: None,
            parent: None,
        };
        let run = offload_core::Run::new(
            id,
            spec,
            offload_core::NodeId::from_bytes([1; 32]),
            Millis(1),
        );
        store.save_run(&run).expect("save");
        (store, id)
    }

    fn log(store: &offload_store::Store, run: RunId, kind: LogKind) -> u64 {
        let event = LogEvent::at(Millis(5), kind);
        u64::try_from(
            store
                .append_event(run, event.kind_name(), event.at_unix_ms, &event)
                .expect("append"),
        )
        .unwrap_or(0)
    }

    #[tokio::test]
    async fn a_pass_delivers_the_news_once_and_the_noise_never() {
        let (store, run) = store_with_a_run();
        let phone = counting("phone", false);
        let sinks = Sinks {
            sinks: vec![phone.clone()],
        };

        // The cursor is created here, at the end of an empty log.
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");

        log(
            &store,
            run,
            LogKind::Text {
                text: "chatter".into(),
            },
        );
        let failed = log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );

        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            1
        );
        // And again, to prove the outbox is what stops a second send rather than luck.
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(30))
                .await
                .expect("pass"),
            0
        );
        assert_eq!(
            *phone
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![failed]
        );
    }

    #[tokio::test]
    async fn one_broken_sink_does_not_stop_another_and_is_given_up_on_loudly() {
        let (store, run) = store_with_a_run();
        let phone = counting("phone", true);
        let laptop = counting("laptop", false);
        let sinks = Sinks {
            sinks: vec![phone.clone(), laptop.clone()],
        };
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );

        // First pass: the laptop gets it, the phone does not.
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            1
        );
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 1);

        // Retried, then given up on — and the reason survives, which is the point.
        for tick in 2..=MAX_ATTEMPTS {
            tend_deliveries(&store, &sinks, None, None, Millis(20 + u64::from(tick)))
                .await
                .expect("pass");
        }
        assert!(store.pending_deliveries(10).expect("pending").is_empty());
        let totals = store.sink_totals("phone").expect("totals");
        assert_eq!((totals.delivered, totals.gave_up), (0, 1));
        assert_eq!(
            totals.last_error.as_deref(),
            Some("gave up: the phone is off")
        );
        assert_eq!(
            phone
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            MAX_ATTEMPTS as usize
        );
    }

    /// A fleet with a fixed list of routes and a switch for whether they answer.
    #[derive(Debug)]
    struct Peers {
        routes: Vec<Route>,
        /// It answers, and says its route failed.
        fail: bool,
        /// It does not answer at all (no address, a timeout): a device that is away.
        silent: std::sync::atomic::AtomicBool,
        /// Nodes it has revoked, and nodes it lists that offer no route.
        revoked: Vec<offload_core::NodeId>,
        listed: Vec<offload_core::NodeId>,
        me: Option<offload_core::NodeId>,
        sent: std::sync::Mutex<Vec<(String, u64)>>,
    }

    #[async_trait::async_trait]
    impl Fleet for Peers {
        fn remote_routes(&self) -> Vec<Route> {
            self.routes.clone()
        }
        fn me(&self) -> Option<offload_core::NodeId> {
            self.me
        }
        fn standing(&self, node: offload_core::NodeId) -> Standing {
            if self.revoked.contains(&node) {
                Standing::Revoked
            } else if self.routes.iter().any(|r| r.node == Some(node))
                || self.listed.contains(&node)
            {
                Standing::Listed
            } else {
                Standing::Unheard
            }
        }
        async fn send(
            &self,
            node: offload_core::NodeId,
            sink: &str,
            note: &Notification,
        ) -> Result<(), SendFailure> {
            self.sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((format!("{}/{sink}", node.short()), note.seq));
            if self.silent.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(SendFailure::Unanswered("the phone did not answer".into()));
            }
            if self.fail {
                return Err(SendFailure::Refused("the phone's route failed".into()));
            }
            Ok(())
        }
    }

    fn peer_route(node: u8, id: &str) -> Route {
        peer_route_for(node, id, offload_core::Service::Push)
    }

    fn peer_route_for(node: u8, id: &str, service: offload_core::Service) -> Route {
        Route {
            node: Some(offload_core::NodeId::from_bytes([node; 32])),
            id: id.into(),
            service: Some(service),
            node_name: format!("node-{node}"),
            reachable: true,
            unusable: None,
            fires: None,
        }
    }

    fn peers(routes: Vec<Route>, fail: bool) -> Peers {
        Peers {
            routes,
            fail,
            silent: std::sync::atomic::AtomicBool::new(false),
            revoked: Vec::new(),
            listed: Vec::new(),
            me: None,
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// A fleet of one peer, offering one route it says works.
    fn view_with_a_peer_route(
        id: &str,
        service: offload_core::Service,
    ) -> offload_core::ClusterView {
        let local = offload_core::NodeId::from_bytes([1; 32]);
        let peer = offload_core::NodeId::from_bytes([2; 32]);
        let mut capabilities = offload_core::Capabilities::empty(
            offload_core::Os::Linux,
            offload_core::Arch::Aarch64,
            offload_core::DeviceClass::Phone,
        );
        let mut capability = offload_core::Capability::new(
            format!("sink:{id}"),
            service,
            [offload_core::Role::Sink],
        );
        capability.authenticated = true;
        capabilities.add(capability);
        let mut view = offload_core::ClusterView::new(local);
        view.upsert_node(
            offload_core::NodeView::new(
                peer,
                capabilities,
                offload_core::WorkPolicy::for_class(offload_core::DeviceClass::Phone),
                Millis(0),
            )
            .named("phone"),
        );
        view
    }

    /// The watermark ADR-0021 prunes behind, in the four states that decide it.
    ///
    /// The two that are easy to get backwards are the absences. A route with **no cursor row**
    /// must not hold anything back — a new cursor initialises to the end of the log, so that
    /// route will never want these events, and reading its absence as zero would stop every
    /// prune on the node. A route that has **gone from the fleet** must not be consulted at all:
    /// its cursor is frozen where it stopped, so it would pin every record for ever.
    #[test]
    fn the_scan_watermark_is_the_lowest_cursor_among_the_routes_that_exist() {
        let store = offload_store::Store::open_memory().expect("store");
        let config = config_with(vec![sink("desk", "/bin/true"), sink("mail", "/bin/true")]);

        assert_eq!(
            scanned_to(&config_with(Vec::new()), &store, None),
            u64::MAX,
            "no routes at all means nothing can ever be owed"
        );
        assert_eq!(
            scanned_to(&config, &store, None),
            u64::MAX,
            "a configured route that has never scanned will start from the end of the log"
        );

        // `sink_cursor` is what creates one, initialising it to the end of the log — which is
        // the behaviour the paragraph above turns on, so the fixture goes through it.
        for (key, seq) in [("desk", 90), ("mail", 40)] {
            store.sink_cursor(key, Topic::Run, Millis(0)).expect("new");
            store.advance_sink(key, Topic::Run, seq).expect("advance");
        }
        assert_eq!(
            scanned_to(&config, &store, None),
            40,
            "the slowest route decides: an event above it is one somebody has not been shown"
        );

        // A peer's route counts while the peer is in the view...
        let view = view_with_a_peer_route("phone", offload_core::Service::Push);
        let key = format!("{}/phone", offload_core::NodeId::from_bytes([2; 32]));
        store.sink_cursor(&key, Topic::Run, Millis(0)).expect("new");
        store.advance_sink(&key, Topic::Run, 5).expect("advance");
        assert_eq!(scanned_to(&config, &store, Some(&view)), 5);

        // ...and stops the moment it is not, cursor row and all. Otherwise a phone that left the
        // fleet last week is a watermark that never advances again.
        assert_eq!(
            scanned_to(&config, &store, None),
            40,
            "a cursor left behind by a route that is gone is not a route that is waiting"
        );
    }

    #[test]
    fn asking_for_a_route_the_fleet_does_not_have_is_answered_at_the_keyboard() {
        // ADR-0014's argument on the other plane: the alternative to saying this now is somebody
        // finding out at breakfast that nothing was ever going to tell them.
        let push = offload_core::Audience::Service {
            service: offload_core::Service::Push,
        };
        let nothing = config_with(Vec::new());
        let note = audience_note(&push, &nothing, None).expect("a note");
        assert!(note.contains("no push route in this fleet"), "{note}");

        // A local route that offers it.
        let mut mine = sink("toast", "true");
        mine.service = "push".into();
        let note = audience_note(&push, &config_with(vec![mine.clone()]), None).expect("a note");
        assert!(note.contains("push goes to toast"), "{note}");

        // A local route that offers it and cannot be used is a different answer with a fix
        // attached — not the same silence as having no route at all.
        let mut broken = mine.clone();
        broken.command = "/definitely/not/here".into();
        let note = audience_note(&push, &config_with(vec![broken]), None).expect("a note");
        assert!(note.contains("cannot be used"), "{note}");

        // And a route on somebody else's device counts, which is the whole point of the plane.
        let view = view_with_a_peer_route("toast", offload_core::Service::Push);
        let note = audience_note(&push, &nothing, Some(&view)).expect("a note");
        assert!(note.contains("phone/toast"), "{note}");
        // ...for its problems: that it finished goes only to this device's routes (ADR-0073).
        assert!(
            note.contains("that it finished, only to no device: this one has no push route"),
            "{note}"
        );
        let note =
            audience_note(&push, &config_with(vec![mine.clone()]), Some(&view)).expect("a note");
        assert!(note.contains("that it finished, only to toast"), "{note}");
        // ...but only for what it actually offers.
        let mail = offload_core::Audience::Service {
            service: offload_core::Service::Email,
        };
        let note = audience_note(&mail, &nothing, Some(&view)).expect("a note");
        assert!(note.contains("no email route"), "{note}");

        // The default says nothing at all: a sentence on every submission is one nobody reads
        // on the submission that matters.
        assert_eq!(
            audience_note(&offload_core::Audience::Everyone, &nothing, Some(&view)),
            None
        );
        // A chosen silence is said back, for the deadline's reason: it changes what happens and
        // there is no later event to point at.
        let quiet =
            audience_note(&offload_core::Audience::Nobody, &nothing, Some(&view)).expect("a note");
        assert!(quiet.contains("nobody will be told"), "{quiet}");
    }

    #[tokio::test]
    async fn a_run_that_asked_for_push_is_not_mailed_as_well() {
        // The gap this closes: every authenticated route used to carry everything, so a fleet
        // with three routes told somebody three times.
        let (store, run) = store_with_run_for(offload_core::Audience::Service {
            service: offload_core::Service::Push,
        });
        let phone = counting_as("phone", offload_core::Service::Push);
        let mailbox = counting_as("mailbox", offload_core::Service::Email);
        let sinks = Sinks {
            sinks: vec![phone.clone(), mailbox.clone()],
        };
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");

        let failed = log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            1
        );
        assert_eq!(*seen(&phone), vec![failed]);
        assert!(seen(&mailbox).is_empty());

        // And the route the run was not for is not *owed* anything either: an outbox row is a
        // promise to deliver, so the decision belongs at the moment the news is noticed.
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 0);
        // Its cursor still moved, because a cursor bounds the scan rather than recording what
        // was sent — otherwise the mailbox re-reads the same entries for ever.
        assert_eq!(
            store
                .existing_sink_cursor("mailbox", Topic::Run)
                .expect("cursor"),
            Some(failed)
        );
    }

    #[tokio::test]
    async fn a_watcher_is_quiet_about_a_firing_that_went_fine_and_loud_about_one_that_did_not() {
        // ADR-0026, and the measurement that made it necessary: a rule firing every three
        // seconds delivered **11 notifications in 40 seconds**, every one "finished after 2
        // turn(s), $0.0005" — and the only way to stop it, `--notify nobody`, delivered **0 of
        // 11 failures** on the same rule failing every firing. Two settings, both wrong, because
        // `Audience` selects routes and this question is about kinds.
        //
        // Both directions in one test, because either half alone passes for the wrong reason: a
        // filter that drops everything looks identical to the fix on the good-news case.
        let (store, run) = store_with_run_wanting(
            offload_core::Audience::Everyone,
            offload_core::Notices::Problems,
        );
        let phone = counting("phone", false);
        let sinks = Sinks {
            sinks: vec![phone.clone()],
        };
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");

        let finished = log(
            &store,
            run,
            LogKind::Finished {
                turns: 2,
                cost_micro_usd: 500,
                denials: 0,
                success: true,
                work: offload_core::WorkKind::Agent,
                result: None,
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            0,
            "a firing that went fine is not worth interrupting anybody for"
        );
        assert!(seen(&phone).is_empty());
        // And nothing is *owed* either — the decision belongs where the news is noticed, so no
        // outbox row was ever written. A filter at send time would leave a promise behind.
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 0);
        // …while the cursor still moved past it, or the scan re-reads it for ever.
        assert_eq!(
            store
                .existing_sink_cursor("phone", Topic::Run)
                .expect("cursor"),
            Some(finished)
        );

        // The commonest failure of all, and the one this test used not to contain: the agent
        // runs, reports `is_error`, and the run is written `Failed` — through the *same*
        // `LogKind::Finished` variant, so the event log's `kind` column says `finished` for it.
        // A filter over that column was quiet about it, which is `Problems` behaving exactly
        // like `Audience::Nobody` in the one case ADR-0026 exists for. Measured on a real
        // daemon: 22 firings, every one failing, 0 notifications.
        let reported = log(
            &store,
            run,
            LogKind::Finished {
                turns: 2,
                cost_micro_usd: 500,
                denials: 0,
                success: false,
                work: offload_core::WorkKind::Agent,
                result: None,
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(25))
                .await
                .expect("pass"),
            1,
            "an agent reporting its own failure is a failure"
        );
        assert_eq!(*seen(&phone), vec![reported]);

        // The half `--notify nobody` threw away, and the whole reason a watcher exists.
        let failed = log(
            &store,
            run,
            LogKind::Failed {
                reason: "the agent stopped without reporting a result".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(30))
                .await
                .expect("pass"),
            1,
            "a failure is the news the rule was written for"
        );
        assert_eq!(*seen(&phone), vec![reported, failed]);

        // …as are the other two requests for attention: a missed deadline and a question.
        let overdue = log(
            &store,
            run,
            LogKind::Overdue {
                by_ms: 1_000,
                waiting: offload_core::Waiting::Placement { refused_by: 0 },
            },
        );
        let asked = log(
            &store,
            run,
            LogKind::Asked {
                tool: "Bash".into(),
                detail: "rm -rf /".into(),
                within_ms: 300_000,
                tool_use_id: "toolu_1".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(40))
                .await
                .expect("pass"),
            2
        );
        assert_eq!(*seen(&phone), vec![reported, failed, overdue, asked]);
    }

    #[tokio::test]
    async fn a_run_somebody_is_watching_interrupts_nobody() {
        let (store, run) = store_with_run_for(offload_core::Audience::Nobody);
        let phone = counting("phone", false);
        let sinks = Sinks {
            sinks: vec![phone.clone()],
        };
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");
        let failed = log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            0
        );
        assert!(seen(&phone).is_empty());
        // The news is still in the log and the cursor has passed it: this decides who is
        // interrupted, never what is recorded.
        assert_eq!(
            store
                .existing_sink_cursor("phone", Topic::Run)
                .expect("cursor"),
            Some(failed)
        );
    }

    #[tokio::test]
    async fn a_route_that_never_said_what_it_is_carries_the_default_and_nothing_named() {
        let unnamed = Arc::new(Counting {
            id: "script".into(),
            fail: false,
            service: None,
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let sinks = Sinks {
            sinks: vec![unnamed.clone() as Arc<dyn Sink>],
        };

        // Asked for by name: it cannot be, because it never made the claim.
        let (store, run) = store_with_run_for(offload_core::Audience::Service {
            service: offload_core::Service::Push,
        });
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            0
        );

        // Nobody routing anything: it works exactly as it did before audiences existed, which
        // is what stops a misspelled `service =` from silently turning a route off.
        let (store, run) = store_with_a_run();
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, None, None, Millis(20))
                .await
                .expect("pass"),
            1
        );
    }

    #[tokio::test]
    async fn a_peers_route_is_chosen_by_what_it_says_it_is() {
        // The fleet half: this node hosts the run and has no route at all, and the audience is
        // matched against what peers advertise — which is all that ever travels.
        let (store, run) = store_with_run_for(offload_core::Audience::Service {
            service: offload_core::Service::Push,
        });
        let sinks = Sinks { sinks: Vec::new() };
        let fleet = peers(
            vec![
                peer_route_for(2, "toast", offload_core::Service::Push),
                peer_route_for(3, "mailbox", offload_core::Service::Email),
            ],
            false,
        );
        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(10))
            .await
            .expect("pass");
        let failed = log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(20))
                .await
                .expect("pass"),
            1
        );
        let sent = fleet
            .sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].0.ends_with("/toast"), "{sent:?}");
        assert_eq!(sent[0].1, failed);
    }

    #[tokio::test]
    async fn a_run_that_finishes_here_is_announced_by_the_node_that_can_reach_somebody() {
        // The point of the whole plane: this node hosts the run and has no route of its own.
        let (store, run) = store_with_a_run();
        let sinks = Sinks { sinks: Vec::new() };
        let fleet = peers(vec![peer_route(2, "phone")], false);

        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(10))
            .await
            .expect("pass");
        let seq = log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(20))
                .await
                .expect("pass"),
            1
        );
        assert_eq!(
            *fleet
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![("02020202/phone".to_string(), seq)]
        );
    }

    /// ADR-0073: every phone with the app is a route, and before this every run anybody started
    /// buzzed every device when it finished. Good news goes to the device the run was submitted
    /// from; a failure still goes to every route its audience allows.
    #[tokio::test]
    async fn that_a_run_finished_goes_to_the_device_it_came_from_and_a_failure_to_all() {
        let (store, run) = store_with_a_run(); // submitted on node 01…
        let sinks = Sinks { sinks: Vec::new() };
        let fleet = Peers {
            // This node (03…) holds the run and has no route; both phones do.
            me: Some(offload_core::NodeId::from_bytes([3; 32])),
            ..peers(vec![peer_route(1, "app"), peer_route(2, "app")], false)
        };
        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(10))
            .await
            .expect("pass");
        let finished = log(
            &store,
            run,
            LogKind::Finished {
                turns: 2,
                cost_micro_usd: 500,
                denials: 0,
                success: true,
                work: offload_core::WorkKind::Agent,
                result: None,
            },
        );
        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(20))
            .await
            .expect("pass");
        let failed = log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(30))
            .await
            .expect("pass");
        let mut sent = fleet
            .sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        sent.sort();
        assert_eq!(
            sent,
            vec![
                ("01010101/app".to_string(), finished),
                ("01010101/app".to_string(), failed),
                ("02020202/app".to_string(), failed),
            ],
            "the finish to the submitter only, the failure to both"
        );
    }

    #[tokio::test]
    async fn two_devices_that_both_call_a_route_phone_are_two_routes() {
        // The outbox key is qualified by the full node id, and it has to be: conflating them
        // would mean telling one person twice and the other never.
        let (store, run) = store_with_a_run();
        let sinks = Sinks {
            sinks: vec![counting("phone", false)],
        };
        let fleet = peers(vec![peer_route(2, "phone"), peer_route(3, "phone")], false);

        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        assert_eq!(
            tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(20))
                .await
                .expect("pass"),
            3,
            "one local route and two peers' routes are three deliveries"
        );
    }

    #[tokio::test]
    async fn a_route_that_is_only_asleep_keeps_its_notification_indefinitely() {
        // The overnight promise, and the thing that was wrong the first time this was built: a
        // phone asleep for a minute used to exhaust its retries and the news was thrown away.
        // Nothing is attempted and nothing is spent while a device is away.
        let (store, run) = store_with_a_run();
        let sinks = Sinks { sinks: Vec::new() };
        let mut asleep = peer_route(2, "phone");
        asleep.reachable = false;
        let away = peers(vec![asleep], false);

        tend_deliveries(&store, &sinks, Some(&away), None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );

        // Many passes — more than `MAX_ATTEMPTS` — and it is still waiting.
        for tick in 0..(MAX_ATTEMPTS + 3) {
            tend_deliveries(
                &store,
                &sinks,
                Some(&away),
                None,
                Millis(20 + u64::from(tick)),
            )
            .await
            .expect("pass");
        }
        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(pending.len(), 1, "still owed");
        assert_eq!(
            pending[0].attempts, 0,
            "nothing was spent on a device that never heard it"
        );
        assert!(away
            .sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());

        // It wakes up.
        let awake = peers(vec![peer_route(2, "phone")], false);
        assert_eq!(
            tend_deliveries(&store, &sinks, Some(&awake), None, Millis(99))
                .await
                .expect("pass"),
            1
        );
        assert!(store.pending_deliveries(10).expect("pending").is_empty());
    }

    #[tokio::test]
    async fn a_route_whose_holder_says_it_cannot_deliver_keeps_its_queue() {
        // The third state, between working and gone. A peer's push credential stops verifying —
        // the script was moved, the token expired — and the route is still listed, still on the
        // next line of `offload sinks`. Everything already queued for it used to be given up on
        // at the next pass, with the reason recorded as "this route no longer exists in the
        // fleet", which was not true of a route the report was showing.
        let (store, run) = store_with_a_run();
        let sinks = Sinks { sinks: Vec::new() };
        let working = peers(vec![peer_route(2, "phone")], true);

        // One failed attempt while it worked, so there is something owed.
        tend_deliveries(&store, &sinks, Some(&working), None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        tend_deliveries(&store, &sinks, Some(&working), None, Millis(20))
            .await
            .expect("pass");
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 1);

        // Now the peer says its credential does not verify. Many passes.
        let mut broken = peer_route(2, "phone");
        broken.unusable = Some("its credential does not verify".into());
        let saying_so = peers(vec![broken], false);
        for tick in 0..(MAX_ATTEMPTS + 3) {
            tend_deliveries(
                &store,
                &sinks,
                Some(&saying_so),
                None,
                Millis(30 + u64::from(tick)),
            )
            .await
            .expect("pass");
        }

        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(pending.len(), 1, "still owed");
        assert_eq!(
            pending[0].attempts, 1,
            "and nothing more was spent: we never asked it"
        );
        assert!(saying_so
            .sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        let totals = store
            .sink_totals(&peer_route(2, "phone").key())
            .expect("totals");
        assert_eq!((totals.delivered, totals.gave_up), (0, 0));

        // Fixed. The news that was waiting goes.
        let fixed = peers(vec![peer_route(2, "phone")], false);
        assert_eq!(
            tend_deliveries(&store, &sinks, Some(&fixed), None, Millis(99))
                .await
                .expect("pass"),
            1
        );
    }

    #[test]
    fn a_route_the_report_lists_is_a_route_the_pass_has_heard_of() {
        // The two used to derive the fleet's routes separately, and disagreed about exactly this
        // case: `offload sinks` listed an unauthenticated peer route and the pass treated it as
        // one that did not exist. One walk over the view now answers both.
        let mut view = view_with_a_peer_route("toast", offload_core::Service::Push);
        let store = offload_store::Store::open_memory().expect("store");

        let listed = fleet_routes(&view, &store);
        let known = routes_in(&view);
        assert_eq!(listed.len(), 1);
        assert_eq!(known.len(), 1);
        assert!(listed[0].authenticated);
        assert_eq!(known[0].unusable, None);

        // The peer's credential stops verifying. Still listed, still known — and now the pass
        // has a reason to give instead of a route it cannot find.
        let peer = offload_core::NodeId::from_bytes([2; 32]);
        let mut node = view.node(&peer).expect("peer").clone();
        let mut capability = node
            .capabilities
            .playing(offload_core::Role::Sink)
            .next()
            .expect("the route")
            .clone();
        capability.authenticated = false;
        node.capabilities.add(capability);
        view.upsert_node(node);

        let listed = fleet_routes(&view, &store);
        let known = routes_in(&view);
        assert_eq!(listed.len(), 1, "the report still explains it");
        assert!(!listed[0].authenticated);
        assert_eq!(known.len(), 1, "and the pass still has the route");
        assert!(known[0].unusable.is_some());
    }

    /// A queued row whose route is not listed any more: gone only if its device was removed from
    /// the fleet or is listed and no longer offers the route. A device this node has not heard of
    /// since it started is waited for — after a restart that is nearly everybody, and taking it for
    /// "gone" dropped the emulator's news on the laptop (session ninety-four).
    #[tokio::test]
    async fn a_missing_route_is_given_up_on_only_when_its_device_is_gone() {
        // One row owed to node 2's phone, whatever happens next.
        async fn owed() -> offload_store::Store {
            let (store, run) = store_with_a_run();
            let sinks = Sinks { sinks: Vec::new() };
            let present = peers(vec![peer_route(2, "phone")], true);
            tend_deliveries(&store, &sinks, Some(&present), None, Millis(10))
                .await
                .expect("pass");
            log(
                &store,
                run,
                LogKind::Failed {
                    reason: "oh".into(),
                },
            );
            tend_deliveries(&store, &sinks, Some(&present), None, Millis(20))
                .await
                .expect("pass");
            assert_eq!(store.pending_deliveries(10).expect("pending").len(), 1);
            store
        }
        let phone = offload_core::NodeId::from_bytes([2; 32]);
        let sinks = Sinks { sinks: Vec::new() };
        let key = peer_route(2, "phone").key();

        // Not heard of since this node started: waits, and says why.
        let store = owed().await;
        let unheard = peers(vec![peer_route(9, "other")], false);
        for tick in 0..(MAX_ATTEMPTS * 2) {
            tend_deliveries(
                &store,
                &sinks,
                Some(&unheard),
                None,
                Millis(30 + u64::from(tick)),
            )
            .await
            .expect("pass");
        }
        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(
            pending.len(),
            1,
            "still owed to a device nobody has said is gone"
        );
        assert_eq!(
            pending[0].last_error.as_deref(),
            Some(
                format!(
                    "{} has not been heard from since this node started; waiting for it",
                    phone.short()
                )
                .as_str()
            )
        );
        assert_eq!(store.sink_totals(&key).expect("totals").gave_up, 0);

        // Removed from the fleet: gone for good.
        let store = owed().await;
        let revoked = Peers {
            revoked: vec![phone],
            ..peers(vec![peer_route(9, "other")], false)
        };
        tend_deliveries(&store, &sinks, Some(&revoked), None, Millis(30))
            .await
            .expect("pass");
        assert!(store.pending_deliveries(10).expect("pending").is_empty());
        let totals = store.sink_totals(&key).expect("totals");
        assert_eq!((totals.delivered, totals.gave_up), (0, 1));
        assert_eq!(
            totals.last_error.as_deref(),
            Some("gave up: its device has been removed from the fleet")
        );

        // Listed, and no longer offering the route: its owner took it away.
        let store = owed().await;
        let withdrawn = Peers {
            listed: vec![phone],
            ..peers(vec![peer_route(9, "other")], false)
        };
        tend_deliveries(&store, &sinks, Some(&withdrawn), None, Millis(30))
            .await
            .expect("pass");
        assert!(store.pending_deliveries(10).expect("pending").is_empty());
        assert_eq!(
            store
                .sink_totals(&key)
                .expect("totals")
                .last_error
                .as_deref(),
            Some("gave up: its device no longer offers this route")
        );
    }

    /// A peer that does not answer is away, not refusing: its news waits however many passes go
    /// by, spends no attempt, and is delivered the first time it answers. The case that was lost:
    /// a node that had just restarted held the stopped phone as alive, every send found no address,
    /// and three passes later the news was abandoned (session ninety-four).
    #[tokio::test]
    async fn a_peer_that_does_not_answer_is_waited_for_and_never_given_up_on() {
        let (store, run) = store_with_a_run();
        let sinks = Sinks { sinks: Vec::new() };
        let fleet = peers(vec![peer_route(2, "phone")], false);
        fleet
            .silent
            .store(true, std::sync::atomic::Ordering::Relaxed);

        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        for tick in 0..(MAX_ATTEMPTS * 3) {
            tend_deliveries(
                &store,
                &sinks,
                Some(&fleet),
                None,
                Millis(20 + u64::from(tick)),
            )
            .await
            .expect("pass");
        }
        let pending = store.pending_deliveries(10).expect("pending");
        assert_eq!(
            pending.len(),
            1,
            "still owed, after {} passes",
            MAX_ATTEMPTS * 3
        );
        assert_eq!(
            pending[0].attempts, 0,
            "no attempt spent on a device that did not answer"
        );
        assert_eq!(
            pending[0].last_error.as_deref(),
            Some("the phone did not answer; waiting for it")
        );
        let totals = store
            .sink_totals(&peer_route(2, "phone").key())
            .expect("totals");
        assert_eq!((totals.delivered, totals.gave_up), (0, 0));

        // It answers.
        fleet
            .silent
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let delivered = tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(100))
            .await
            .expect("pass");
        assert_eq!(delivered, 1);
        assert!(store.pending_deliveries(10).expect("pending").is_empty());
    }

    #[tokio::test]
    async fn a_peer_that_will_not_carry_it_is_retried_and_then_given_up_on() {
        // Exactly as a local route is: from the sender's outbox, which is the only place that
        // knows what has been said. The peer answers and forgets.
        let (store, run) = store_with_a_run();
        let sinks = Sinks { sinks: Vec::new() };
        let fleet = peers(vec![peer_route(2, "phone")], true);

        tend_deliveries(&store, &sinks, Some(&fleet), None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        for tick in 0..=MAX_ATTEMPTS {
            tend_deliveries(
                &store,
                &sinks,
                Some(&fleet),
                None,
                Millis(20 + u64::from(tick)),
            )
            .await
            .expect("pass");
        }
        assert!(store.pending_deliveries(10).expect("pending").is_empty());
        let totals = store
            .sink_totals(&peer_route(2, "phone").key())
            .expect("totals");
        assert_eq!(totals.gave_up, 1);
        assert_eq!(
            totals.last_error.as_deref(),
            Some("gave up: the phone's route failed")
        );
    }

    #[test]
    fn a_route_is_named_by_the_capability_its_holder_advertises() {
        // One convention, written in `sink_capabilities` and read here. Two copies of it is one
        // copy that will be wrong.
        assert_eq!(sink_id_of("sink:phone"), "phone");
        assert_eq!(sink_id_of("phone"), "phone", "tolerates either spelling");
        let caps = sink_capabilities(&config_with(vec![sink("phone", "true")]));
        assert_eq!(sink_id_of(&caps[0].id.0), "phone");
    }

    #[tokio::test]
    async fn a_sink_removed_from_the_config_stops_being_retried() {
        let (store, run) = store_with_a_run();
        let sinks = Sinks {
            sinks: vec![counting("phone", true)],
        };
        tend_deliveries(&store, &sinks, None, None, Millis(10))
            .await
            .expect("pass");
        log(
            &store,
            run,
            LogKind::Failed {
                reason: "oh".into(),
            },
        );
        tend_deliveries(&store, &sinks, None, None, Millis(20))
            .await
            .expect("pass");
        assert_eq!(store.pending_deliveries(10).expect("pending").len(), 1);

        // The owner deleted the sink from node.toml and restarted.
        let empty = Sinks { sinks: Vec::new() };
        tend_deliveries(&store, &empty, None, None, Millis(30))
            .await
            .expect("pass");
        assert_eq!(
            store.pending_deliveries(10).expect("pending").len(),
            1,
            "a node with no sinks at all does no work, so nothing is resolved either"
        );

        let other = Sinks {
            sinks: vec![counting("laptop", false)],
        };
        tend_deliveries(&store, &other, None, None, Millis(40))
            .await
            .expect("pass");
        assert!(
            store.pending_deliveries(10).expect("pending").is_empty(),
            "the orphaned row is closed out with its reason rather than retried for ever"
        );
    }
}
