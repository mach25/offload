//! What a device *is and has*.
//!
//! Capabilities describe ability, not permission. What the device's owner is willing to let
//! it do lives in [`crate::policy::WorkPolicy`]. Both gate eligibility, and keeping them
//! apart is what lets a phone say "I can run this, but not right now" and have the
//! scheduler print a useful reason.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    Linux,
    MacOs,
    Windows,
    Android,
    Ios,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    X86_64,
    Aarch64,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClass {
    Phone,
    Tablet,
    Laptop,
    Desktop,
    Server,
    Vm,
    Unknown,
}

/// How much the scheduler should expect this node to still be here in an hour.
///
/// Affects *scoring*, never eligibility — an `Ephemeral` phone is a perfectly good worker
/// for a short run. Ordering matters: `Ephemeral < Transient < Stable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    /// Comes and goes on the order of minutes: phone, tablet.
    Ephemeral,
    /// Hours; lid closes, network roams: laptop.
    Transient,
    /// Days or weeks, on mains power: desktop, server, VM.
    Stable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PowerSource {
    Battery { percent: u8, charging: bool },
    Ac,
    Unknown,
}

// The spellings `offload match`, `--require` and `--prefer` accept, so a refusal names a value
// the way the person typed it. `Constraint::explain` used `{:?}` and printed `os == MacOs (have
// Linux)` under `--require os=macos`, and `have Battery { percent: 57, charging: false }` for
// the one clause a phone fails most.
impl std::fmt::Display for Os {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Os::Linux => "linux",
            Os::MacOs => "macos",
            Os::Windows => "windows",
            Os::Android => "android",
            Os::Ios => "ios",
            Os::Other(other) => other,
        })
    }
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::Other(other) => other,
        })
    }
}

impl std::fmt::Display for DeviceClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DeviceClass::Phone => "phone",
            DeviceClass::Tablet => "tablet",
            DeviceClass::Laptop => "laptop",
            DeviceClass::Desktop => "desktop",
            DeviceClass::Server => "server",
            DeviceClass::Vm => "vm",
            DeviceClass::Unknown => "unknown",
        })
    }
}

impl std::fmt::Display for Stability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Stability::Ephemeral => "ephemeral",
            Stability::Transient => "transient",
            Stability::Stable => "stable",
        })
    }
}

impl std::fmt::Display for PowerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PowerSource::Battery {
                percent,
                charging: true,
            } => write!(f, "battery at {percent}%, charging"),
            PowerSource::Battery {
                percent,
                charging: false,
            } => write!(f, "battery at {percent}%, not charging"),
            PowerSource::Ac => f.write_str("mains"),
            PowerSource::Unknown => f.write_str("unknown power"),
        }
    }
}

impl PowerSource {
    #[must_use]
    pub fn on_mains(&self) -> bool {
        match self {
            PowerSource::Ac => true,
            PowerSource::Battery { charging, .. } => *charging,
            PowerSource::Unknown => false,
        }
    }

    /// `None` when the device has no battery to speak of.
    #[must_use]
    pub fn battery_percent(&self) -> Option<u8> {
        match self {
            PowerSource::Battery { percent, .. } => Some(*percent),
            _ => None,
        }
    }
}

/// Whether this device's bytes cost money — and whether anybody actually knows (ADR-0045).
///
/// Three-valued for [`PowerSource`]'s reason, four fields up: a device nobody could ask about is
/// not the same as a device that answered. This was a `bool` set unconditionally to `false` by the
/// probe, under a comment that called it an assumption, and four readers acted on it as a fact —
/// so `Refusal::MeteredNetwork` was a sentence no fleet could produce and `--require unmetered`
/// matched every node in the world.
///
/// **`Unknown` is not resolved once, here.** Each reader decides what it means, because the answer
/// depends on which way the claim points: a policy refusing work claims the bytes cost something,
/// a constraint requiring an unmetered link claims they do not, and `Unknown` proves neither. See
/// ADR-0045 §4 — the burden of proof lies with whoever is making the claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metered {
    Yes,
    No,
    Unknown,
}

impl Metered {
    /// Is this link *known* to cost money? `Unknown` is not, so nothing refuses on it.
    #[must_use]
    pub fn known_metered(self) -> bool {
        self == Metered::Yes
    }

    /// Is this link *known* to be free? `Unknown` is not, so `--require unmetered` does not match.
    #[must_use]
    pub fn known_unmetered(self) -> bool {
        self == Metered::No
    }
}

impl std::fmt::Display for Metered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Metered::Yes => "yes",
            Metered::No => "no",
            Metered::Unknown => "unknown",
        })
    }
}

/// Which agent is being orchestrated. `Other` keeps third-party adapters representable
/// without a core change.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Other(String),
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentKind::ClaudeCode => f.write_str("claude-code"),
            AgentKind::Other(name) => f.write_str(name),
        }
    }
}

/// Opaque fingerprint of the account an agent is authenticated as.
///
/// Not a credential and not reversible — it exists so the scheduler can tell that three
/// nodes are authenticated as the *same* account and therefore share one rate limit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(pub String);

/// Salt and format version in one, for the two [`AccountId`] derivations below.
///
/// **This is a wire format, not a tuning knob** — the same rule as the fleet key's derivation
/// (`fleet::passphrase`), for a quieter version of the same reason. A fingerprint is only ever
/// compared *across* nodes, so two nodes computing it differently do not disagree loudly: they
/// simply never match, and per-account accounting silently stops happening. Bump the suffix
/// rather than editing anything below it.
const ACCOUNT_SALT: &[u8] = b"offload-account-v1";

/// Hex characters kept from the digest. 64 bits is far more than a personal fleet needs to tell
/// two accounts apart, and short enough to read in the refusal text `offload explain` prints.
const ACCOUNT_DIGEST_HEX: usize = 16;

impl AccountId {
    /// Fingerprint the account an agent is logged in as, from an identifier the agent itself
    /// keeps — for Claude Code, `oauthAccount.accountUuid`.
    ///
    /// Hashed rather than carried, because this value is gossiped to every peer and rendered
    /// into human-facing text: the type promises to be "not a credential and not reversible",
    /// and a uuid has enough entropy that a digest of it keeps that promise. The account's email
    /// address would not — it is guessable, so hashing it would look exactly as opaque while
    /// being reversible by anybody willing to try a list of addresses.
    #[must_use]
    pub fn of_account(account_uuid: &str) -> AccountId {
        AccountId(format!("acct:{}", digest(b"account", account_uuid.trim())))
    }

    /// Fingerprint an API key, for a node authenticated by one rather than by a login.
    ///
    /// The key's *value*, deliberately, rather than the fact that a key is set. Two nodes
    /// holding two different keys are two accounts that share no rate limit, and the label this
    /// replaced — the name of the environment variable — collapsed every such node into one
    /// account, which would have had them share a cap they do not share.
    #[must_use]
    pub fn of_api_key(key: &str) -> AccountId {
        AccountId(format!("acct:{}", digest(b"api-key", key.trim())))
    }
}

/// Domain-separated digest of one secret-ish string. The domain tag is what stops an API key
/// and an account uuid that happen to be the same characters from fingerprinting alike.
fn digest(domain: &[u8], value: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ACCOUNT_SALT);
    hasher.update(b"\0");
    hasher.update(domain);
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize().as_bytes())
        .chars()
        .take(ACCOUNT_DIGEST_HEX)
        .collect()
}

/// What an agent service carries beyond the fields every capability has.
///
/// Was `AgentCapability`, and is now the `details` of a capability whose service is an agent
/// (ADR-0011). `authenticated` and the account moved out to [`Capability`], where every
/// service answers those two questions the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDetails {
    /// As reported by the agent itself. Used for eligibility, because a resume format is
    /// not guaranteed portable to an older agent. See ADR-0003.
    pub version: String,
    /// The models this node's agent says its account can use, in the agent's own order
    /// (ADR-0080). Read from the agent, never assumed: empty when it could not be read.
    pub models: Vec<Model>,
    /// Ceiling this node imposes on itself for this agent.
    pub max_concurrent: u32,
}

impl AgentDetails {
    /// Whether `model` is one this agent offers, named either as `--model` takes it or as the
    /// model it resolves to — `opus` and `claude-opus-5-5` are one model to a person choosing.
    #[must_use]
    pub fn reaches(&self, model: &str) -> bool {
        self.models
            .iter()
            .any(|m| m.value == model || m.resolves_to == model)
    }
}

/// A model an agent offers, as the agent itself describes it (ADR-0080).
///
/// Every field is the agent's own words. `value` is what `--model` takes and may be an alias
/// (`opus`) or a full name; `resolves_to` is the model that alias means today, which is why a
/// list read once goes stale when the agent updates and is read again when its version changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub value: String,
    pub name: String,
    pub about: String,
    pub resolves_to: String,
}

impl Model {
    /// A model known by one name only — for tests and fixtures that do not care about the rest.
    #[must_use]
    pub fn named(value: &str) -> Model {
        Model {
            value: value.to_string(),
            name: value.to_string(),
            about: String::new(),
            resolves_to: value.to_string(),
        }
    }
}

/// What kind of thing a capability is.
///
/// A closed enum with an escape hatch, so the common path is typed and a fleet with something
/// unusual is still representable. ADR-0011 is clear-eyed that `Other` is also how matching
/// degrades into string soup — nothing stops a fleet from living in the escape hatch, and
/// nothing here should encourage it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "service", content = "of")]
pub enum Service {
    /// An agent this node can run. The kind is part of the identity of the service, because
    /// "has an agent" is never the question — "has *this* agent" is.
    Agent(AgentKind),
    /// A terminal a human is watching. The only sink that disappears when they walk away.
    Terminal,
    Push,
    Email,
    /// Slack, Matrix, whatever: the workspace is the instance, so two of them are two
    /// capabilities.
    Chat(String),
    Webhook,
    /// Something that fires on a clock rather than on an event.
    Schedule,
    Other(String),
}

impl std::fmt::Display for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Service::Agent(kind) => write!(f, "agent:{kind}"),
            Service::Terminal => f.write_str("terminal"),
            Service::Push => f.write_str("push"),
            Service::Email => f.write_str("email"),
            Service::Chat(workspace) => write!(f, "chat:{workspace}"),
            Service::Webhook => f.write_str("webhook"),
            Service::Schedule => f.write_str("schedule"),
            Service::Other(name) => f.write_str(name),
        }
    }
}

/// The inverse of [`Service`]'s `Display`, so a person can name a service in a config file.
///
/// It exists *because* the `Display` does. A type with one direction only gets a hand-written
/// parser somewhere else eventually, and then two spellings of the same service — which is
/// exactly the string soup ADR-0011 warns about, arriving by the back door. The round trip is
/// a test.
///
/// Unrecognised names become [`Service::Other`] rather than an error, which is the escape
/// hatch working as designed: a fleet may describe something this build has never heard of.
/// `agent:` is deliberately *not* parsed into an unknown agent — an agent kind that this build
/// cannot spawn is a claim it must not make.
impl std::str::FromStr for Service {
    type Err = UnknownService;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if let Some(kind) = text.strip_prefix("agent:") {
            return match kind {
                "claude-code" => Ok(Service::Agent(AgentKind::ClaudeCode)),
                other => Err(UnknownService(format!("agent:{other}"))),
            };
        }
        if let Some(workspace) = text.strip_prefix("chat:") {
            return Ok(Service::Chat(workspace.to_string()));
        }
        Ok(match text {
            "terminal" => Service::Terminal,
            "push" => Service::Push,
            "email" => Service::Email,
            "webhook" => Service::Webhook,
            "schedule" => Service::Schedule,
            "" => return Err(UnknownService(String::new())),
            other => Service::Other(other.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", unknown_service(.0))]
pub struct UnknownService(pub String);

/// Two sentences, because an empty name is not a wrong name — it is a **missing field**.
///
/// Every config block that carries one (`[[sinks]]`, `[[triggers]]`, `[[resources]]`,
/// `[[tasks]]`) declares `service: String` under `#[serde(default)]`, so leaving the line out
/// produces `""` rather than a deserialisation error. That reached the operator as
/// `invalid config: : not a service this build can offer` — a sentence opening with a colon,
/// about a name that was never written, which reads as a service this build does not know
/// rather than as a line the block is missing. Measured in session seventy-eight against a
/// `[[triggers]]` block written without one: the daemon ignored the trigger, said so, and named
/// everything except the field to add.
fn unknown_service(name: &str) -> String {
    if name.is_empty() {
        "no `service` was given, and one is required".to_string()
    } else {
        format!("{name}: not a service this build can offer")
    }
}

/// What a run may do with a resource it has been granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

/// What a capability can be used *for*.
///
/// ADR-0011 names three, because they are three different grants — and this adds a fourth,
/// `Execute`, for the one the ADR discusses everywhere without listing: hosting a run. It is
/// a grant in exactly the same sense (`Grant::HostRuns` already exists in membership), and
/// leaving agents role-less would mean the one vocabulary had a hole in the middle of it.
///
/// The same service in two roles is two different permissions. Email is the clarifying case:
/// "can send as me@example.com" and "can read everything arriving at me@example.com"
/// are the same integration, the same account, and nothing like the same grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "role")]
pub enum Role {
    /// Runs execute here.
    Execute,
    /// The fleet reaches a human through it. Fire and forget (ADR-0010).
    Sink,
    /// Something arrives and creates or wakes work.
    Trigger,
    /// A run acts on it while executing. The only role that hands a run something, and so
    /// the only one that has to be granted per run rather than implied by placement.
    Resource { access: Access },
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Execute => f.write_str("execute"),
            Role::Sink => f.write_str("sink"),
            Role::Trigger => f.write_str("trigger"),
            Role::Resource { access } => write!(f, "resource:{access:?}"),
        }
    }
}

/// Per-service payload. Kept small: anything that decisions are made on belongs in the typed
/// fields of [`Capability`], not in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "details")]
pub enum ServiceDetails {
    Agent(AgentDetails),
    /// Nothing to add. Sinks and resources grow their own variants when they are built.
    None,
}

impl ServiceDetails {
    #[must_use]
    pub fn agent(&self) -> Option<&AgentDetails> {
        match self {
            ServiceDetails::Agent(details) => Some(details),
            ServiceDetails::None => None,
        }
    }
}

/// Stable name for one capability on one node: `agent:claude-code`, `email:me@example.com`.
///
/// Stable is the operative word. It is how a capability is referred to in a grant, so a node
/// that renames one has revoked it and issued another.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(pub String);

impl CapabilityId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        CapabilityId(id.into())
    }
}

impl std::fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One thing a device can do, as an instance rather than a flag (ADR-0011).
///
/// Instances rather than a map keyed by kind, because two mailboxes are two entries and the
/// old shape could not say so. Everything a decision is made on is typed; `description` is
/// for humans and is never matched, because a free-text field that decisions are made against
/// is `explain` drifting from `matches` permanently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub id: CapabilityId,
    pub service: Service,
    pub roles: BTreeSet<Role>,
    /// Who it acts as, comparable across nodes. Answers "route this to me" and "these two
    /// nodes are one resource" — which is the same fact that makes three nodes on one agent
    /// account share one rate limit.
    pub identity: Option<AccountId>,
    /// Verified, never assumed. An unverifiable credential is `false`: a node that claims a
    /// capability it lacks wins bids and then fails every run it takes.
    pub authenticated: bool,
    pub details: ServiceDetails,
    /// One line, for `offload probe` and refusal messages. Never matched on.
    pub description: String,
}

impl Capability {
    /// A capability with nothing claimed: no identity, no auth, no detail.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        service: Service,
        roles: impl IntoIterator<Item = Role>,
    ) -> Self {
        Capability {
            id: CapabilityId::new(id),
            service,
            roles: roles.into_iter().collect(),
            identity: None,
            authenticated: false,
            details: ServiceDetails::None,
            description: String::new(),
        }
    }

    /// Say this capability cannot be used, and why, without losing the owner's label.
    ///
    /// [`Self::description`] carried two different things depending on who set it: for an agent
    /// whose login does not match its config it was a *reason*, and for the four nominated kinds
    /// — sink, trigger, resource, task — it was the owner's one-line *label*. `offload probe`
    /// prints it as a `└─` clause under a `NOT authenticated` line, and its own code comment
    /// calls that clause "*why*". So a `[[tasks]]` entry pointing at a missing program printed
    ///
    /// ```text
    /// service     task:nightly — NOT authenticated
    ///             └─ posts the nightly report to slack
    /// ```
    ///
    /// — a sentence that reads as the explanation, is not one, and sends somebody to look at the
    /// wrong thing. One field, two facts, the trap this tree keeps meeting. Both now, in one
    /// order, in one place: the label if there is one, then the reason.
    ///
    /// The reason **gossips**, like the rest of a capability, and must therefore be a fact about
    /// what the device can do rather than about how it does it — no paths, which is ADR-0010's
    /// rule and the reason `description` is the owner's words and never the command.
    pub fn unusable(&mut self, reason: &str) {
        self.authenticated = false;
        self.description = match self.description.trim() {
            "" => reason.to_string(),
            label => format!("{label} — {reason}"),
        };
    }

    /// The conventional id for an agent service, so that two nodes name the same thing the
    /// same way.
    #[must_use]
    pub fn agent(kind: AgentKind, details: AgentDetails, authenticated: bool) -> Self {
        let id = format!("agent:{kind}");
        Capability {
            description: format!("{kind} {}", details.version),
            id: CapabilityId::new(id),
            service: Service::Agent(kind),
            roles: [Role::Execute].into_iter().collect(),
            identity: None,
            authenticated,
            details: ServiceDetails::Agent(details),
        }
    }

    #[must_use]
    pub fn with_identity(mut self, identity: Option<AccountId>) -> Self {
        self.identity = identity;
        self
    }

    /// Mark the credential as verified. Never a default: an unverifiable auth is `false`,
    /// because a node claiming capability it lacks wins bids and then fails every run.
    #[must_use]
    pub fn authenticated(mut self) -> Self {
        self.authenticated = true;
        self
    }

    #[must_use]
    pub fn described(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    #[must_use]
    pub fn plays(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }

    /// Is this something a run could be granted, whatever access it offers?
    ///
    /// Asked instead of `plays(Role::Resource { access })` because the two ends of that question
    /// know different halves of it. The *node's owner* knows what their mailbox integration can
    /// do — read, write, both — and the operator submitting a run knows only that the run needs
    /// to reach email. Making the run name an access it cannot know would have it guess, and a
    /// guess that is too narrow is a run that will not place while the resource sits there.
    ///
    /// So access describes the grant rather than selecting it. Narrowing what a run may do with
    /// a resource it has been given is a separate decision, and belongs with the tool projection
    /// rather than with placement.
    #[must_use]
    pub fn is_resource(&self) -> bool {
        self.roles
            .iter()
            .any(|role| matches!(role, Role::Resource { .. }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub os: Os,
    pub arch: Arch,
    pub device_class: DeviceClass,
    pub stability: Stability,

    pub cpu_cores: u32,
    pub memory_mb: u64,
    pub disk_free_mb: u64,
    pub power: PowerSource,
    /// Mobile data, tethering, or anything else where bytes cost money — or `Unknown`, which
    /// is what a device says when nobody could tell (ADR-0045).
    pub metered_network: Metered,

    /// Everything this device can do, keyed by the capability's own id.
    ///
    /// The key and `Capability::id` are the same value; [`Capabilities::add`] is the only way
    /// to insert one, so they cannot drift apart.
    services: BTreeMap<CapabilityId, Capability>,
    /// `rust` -> `1.82.0`, `node` -> `22.3.0`, ...
    pub toolchains: BTreeMap<String, String>,
    /// Free-form facts: `gpu:cuda`, `gh:authed`, `docker`.
    pub tags: BTreeSet<String>,
    /// Operator-assigned key/value metadata: `location=home`, `owner=me`.
    pub labels: BTreeMap<String, String>,
}

impl Capabilities {
    /// Minimal capabilities for a device that can do nothing. Tests and probe fallbacks
    /// build from here rather than from a `Default` that pretends to know things.
    #[must_use]
    pub fn empty(os: Os, arch: Arch, device_class: DeviceClass) -> Self {
        Capabilities {
            os,
            arch,
            stability: DeviceClass::default_stability(device_class),
            device_class,
            cpu_cores: 1,
            memory_mb: 0,
            disk_free_mb: 0,
            power: PowerSource::Unknown,
            metered_network: Metered::Unknown,
            services: BTreeMap::new(),
            toolchains: BTreeMap::new(),
            tags: BTreeSet::new(),
            labels: BTreeMap::new(),
        }
    }

    /// Which fields differ from `other`, by name — for a log line that says what changed rather
    /// than that something did. Every field is listed, destructured so that a new one does not
    /// compile until it is named here too.
    #[must_use]
    pub fn differences(&self, other: &Capabilities) -> Vec<&'static str> {
        let Capabilities {
            os,
            arch,
            device_class,
            stability,
            cpu_cores,
            memory_mb,
            disk_free_mb,
            power,
            metered_network,
            services,
            toolchains,
            tags,
            labels,
        } = self;
        [
            ("os", *os == other.os),
            ("arch", *arch == other.arch),
            ("device_class", *device_class == other.device_class),
            ("stability", *stability == other.stability),
            ("cpu_cores", *cpu_cores == other.cpu_cores),
            ("memory_mb", *memory_mb == other.memory_mb),
            ("disk_free_mb", *disk_free_mb == other.disk_free_mb),
            ("power", *power == other.power),
            ("metered_network", *metered_network == other.metered_network),
            ("services", *services == other.services),
            ("toolchains", *toolchains == other.toolchains),
            ("tags", *tags == other.tags),
            ("labels", *labels == other.labels),
        ]
        .into_iter()
        .filter(|(_, same)| !same)
        .map(|(name, _)| name)
        .collect()
    }

    /// Add a capability, replacing any with the same id.
    pub fn add(&mut self, capability: Capability) -> &mut Self {
        self.services.insert(capability.id.clone(), capability);
        self
    }

    #[must_use]
    pub fn get(&self, id: &CapabilityId) -> Option<&Capability> {
        self.services.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &Capability> {
        self.services.values()
    }

    /// Capabilities offering `service`, in id order. Plural because two mailboxes are two
    /// entries — the whole reason this stopped being a map keyed by kind (ADR-0011).
    pub fn offering<'a>(&'a self, service: &'a Service) -> impl Iterator<Item = &'a Capability> {
        self.services
            .values()
            .filter(move |c| &c.service == service)
    }

    /// Capabilities a run could be granted, whatever their access. See
    /// [`Capability::is_resource`] for why access is not part of the question.
    pub fn resources(&self) -> impl Iterator<Item = &Capability> {
        self.services.values().filter(|c| c.is_resource())
    }

    /// Capabilities playing `role`, whatever the service.
    pub fn playing(&self, role: Role) -> impl Iterator<Item = &Capability> {
        self.services.values().filter(move |c| c.plays(role))
    }

    /// The agent service for `kind`, if this node has one.
    ///
    /// Kept as a named accessor because "can this device run this agent" is the question the
    /// scheduler asks most, and spelling it out at every call site would bury it.
    #[must_use]
    pub fn agent(&self, kind: &AgentKind) -> Option<&Capability> {
        self.services
            .values()
            .find(|c| matches!(&c.service, Service::Agent(k) if k == kind))
    }

    /// The agent's own details — version, models, its self-imposed ceiling.
    #[must_use]
    pub fn agent_details(&self, kind: &AgentKind) -> Option<&AgentDetails> {
        self.agent(kind).and_then(|c| c.details.agent())
    }

    /// Accounts this node is authenticated against, for fleet-wide rate-limit accounting.
    ///
    /// Every service, not only agents: "these two nodes are one mailbox" is the same fact as
    /// "these three nodes share one rate limit", and it gets one answer.
    pub fn accounts(&self) -> impl Iterator<Item = &AccountId> {
        self.services
            .values()
            .filter(|c| c.authenticated)
            .filter_map(|c| c.identity.as_ref())
    }
}

impl DeviceClass {
    /// Starting guess only. A node observed to be reliable should have this revised
    /// upward from measurement rather than assumed from its class — a desktop that sleeps
    /// nightly is not `Stable` in the sense the scheduler means.
    #[must_use]
    pub fn default_stability(self) -> Stability {
        match self {
            DeviceClass::Phone | DeviceClass::Tablet => Stability::Ephemeral,
            DeviceClass::Laptop | DeviceClass::Unknown => Stability::Transient,
            DeviceClass::Desktop | DeviceClass::Server | DeviceClass::Vm => Stability::Stable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words a refusal prints for the one clause a phone fails most — a struct literal,
    /// `Battery { percent: 57, charging: false }`, before these existed.
    #[test]
    fn power_reads_as_words() {
        let low = PowerSource::Battery {
            percent: 57,
            charging: false,
        };
        assert_eq!(low.to_string(), "battery at 57%, not charging");
        assert_eq!(PowerSource::Ac.to_string(), "mains");
        assert_eq!(Os::MacOs.to_string(), "macos");
        assert_eq!(DeviceClass::Vm.to_string(), "vm");
        // The two a config also spells: tied to serde's own rendering, the way `AcceptWork` is,
        // so a renamed variant cannot part them.
        for class in [
            DeviceClass::Phone,
            DeviceClass::Tablet,
            DeviceClass::Laptop,
            DeviceClass::Desktop,
            DeviceClass::Server,
            DeviceClass::Vm,
            DeviceClass::Unknown,
        ] {
            assert_eq!(
                serde_json::to_string(&class).expect("encode"),
                format!("\"{class}\"")
            );
        }
        for stability in [
            Stability::Ephemeral,
            Stability::Transient,
            Stability::Stable,
        ] {
            assert_eq!(
                serde_json::to_string(&stability).expect("encode"),
                format!("\"{stability}\"")
            );
        }
    }

    #[test]
    fn stability_orders_ephemeral_below_stable() {
        assert!(Stability::Ephemeral < Stability::Transient);
        assert!(Stability::Transient < Stability::Stable);
    }

    fn caps() -> Capabilities {
        Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop)
    }

    fn mailbox(address: &str, roles: &[Role]) -> Capability {
        Capability::new(
            format!("email:{address}"),
            Service::Email,
            roles.iter().copied(),
        )
        .with_identity(Some(AccountId(address.to_string())))
    }

    #[test]
    fn two_mailboxes_are_two_capabilities() {
        // The structural limit that prompted ADR-0011: a map keyed by kind could hold one
        // entry per service, so a second mail account was not expressible at all.
        let mut caps = caps();
        caps.add(mailbox("work@example.com", &[Role::Sink]));
        caps.add(mailbox("home@example.com", &[Role::Sink]));

        assert_eq!(caps.offering(&Service::Email).count(), 2);
        assert_eq!(caps.playing(Role::Sink).count(), 2);
    }

    /// The clause `offload probe` prints under a `NOT authenticated` line is *why*, and for
    /// four of the five kinds that set it, it was the owner's label instead.
    #[test]
    fn an_unusable_capability_keeps_its_label_and_gains_the_reason() {
        let mut nominated = Capability::new(
            "task:nightly",
            Service::Other("nightly".into()),
            [Role::Execute],
        );
        nominated.authenticated = true;
        nominated.description = "posts the nightly report to slack".into();
        nominated.unusable("its program is not on this device");
        assert!(!nominated.authenticated);
        assert_eq!(
            nominated.description,
            "posts the nightly report to slack — its program is not on this device"
        );

        // An owner who wrote no label gets the reason alone rather than a leading dash — the
        // arm the agent's account mismatch takes, which never had a label to keep.
        let mut bare = Capability::new("task:bare", Service::Other("bare".into()), [Role::Execute]);
        bare.unusable("its program is not on this device");
        assert_eq!(bare.description, "its program is not on this device");
    }

    #[test]
    fn every_service_survives_being_written_down_and_read_back() {
        // The reason `FromStr` exists at all: a `Display` on its own gets a second,
        // hand-written parser somewhere else, and then two spellings of one service.
        use std::str::FromStr;
        for service in [
            Service::Agent(AgentKind::ClaudeCode),
            Service::Terminal,
            Service::Push,
            Service::Email,
            Service::Chat("team-a".into()),
            Service::Webhook,
            Service::Schedule,
            Service::Other("carrier-pigeon".into()),
        ] {
            let text = service.to_string();
            assert_eq!(Service::from_str(&text), Ok(service), "{text}");
        }

        // An agent this build cannot spawn is a claim it must not make; anything else
        // unrecognised is the escape hatch working as designed.
        assert!(Service::from_str("agent:some-other-agent").is_err());
        // An empty name is a *missing field*, not a wrong one: every config block that carries a
        // service declares it under `#[serde(default)]`, so omitting the line yields `""`. The
        // message used to open with a colon and name nothing the operator could act on.
        let missing = Service::from_str("").expect_err("empty is refused");
        assert_eq!(
            missing.to_string(),
            "no `service` was given, and one is required"
        );
        // The other arm is unchanged. An arbitrary name is a legal `Service::Other`, so the only
        // thing that reaches this sentence is an agent this build does not have.
        assert_eq!(
            Service::from_str("agent:some-other-agent")
                .expect_err("unknown agent is refused")
                .to_string(),
            "agent:some-other-agent: not a service this build can offer"
        );
        assert_eq!(
            Service::from_str("  push  "),
            Ok(Service::Push),
            "config files have whitespace in them"
        );
    }

    #[test]
    fn the_same_service_in_two_roles_is_two_different_grants() {
        // "Can send as this address" and "can read everything arriving at it" are the same
        // integration, the same account, and nothing like the same permission.
        let mut caps = caps();
        caps.add(mailbox("me@example.com", &[Role::Sink]));
        caps.add(
            Capability::new(
                "email:me@example.com:inbox",
                Service::Email,
                [Role::Resource {
                    access: Access::Read,
                }],
            )
            .with_identity(Some(AccountId("me@example.com".into()))),
        );

        assert_eq!(caps.playing(Role::Sink).count(), 1);
        assert_eq!(
            caps.playing(Role::Resource {
                access: Access::Read
            })
            .count(),
            1
        );
        // One identity, two capabilities: the same account reached two different ways.
        assert_eq!(caps.offering(&Service::Email).count(), 2);
    }

    #[test]
    fn re_adding_the_same_id_replaces_rather_than_duplicates() {
        // The id is what a grant names, so two entries under one id would mean a grant that
        // is ambiguous about what it granted.
        let mut caps = caps();
        caps.add(mailbox("me@example.com", &[Role::Sink]));
        caps.add(mailbox("me@example.com", &[Role::Trigger]));

        assert_eq!(caps.all().count(), 1);
        assert!(caps.playing(Role::Trigger).count() == 1);
    }

    #[test]
    fn a_model_is_reached_by_the_name_a_person_gives_or_the_model_it_resolves_to() {
        // ADR-0080: `opus` today means `claude-opus-5-5`, and a person may type either.
        let details = AgentDetails {
            version: "2.1.283".into(),
            models: vec![Model {
                value: "opus".into(),
                name: "Opus 5.5".into(),
                about: String::new(),
                resolves_to: "claude-opus-5-5".into(),
            }],
            max_concurrent: 2,
        };
        assert!(details.reaches("opus"));
        assert!(details.reaches("claude-opus-5-5"));
        assert!(
            !details.reaches("Opus 5.5"),
            "the display name is not a model name"
        );
        assert!(!details.reaches("claude-opus-5"));
    }

    #[test]
    fn an_agent_is_found_by_kind_and_carries_its_own_details() {
        let mut caps = caps();
        caps.add(Capability::agent(
            AgentKind::ClaudeCode,
            AgentDetails {
                version: "2.11.0".into(),
                models: vec![Model::named("claude-opus-5")],
                max_concurrent: 3,
            },
            true,
        ));

        let agent = caps.agent(&AgentKind::ClaudeCode).expect("agent");
        assert!(agent.plays(Role::Execute));
        assert_eq!(
            caps.agent_details(&AgentKind::ClaudeCode)
                .expect("details")
                .version,
            "2.11.0"
        );
        assert!(caps.agent(&AgentKind::Other("codex".into())).is_none());
    }

    #[test]
    fn only_verified_credentials_count_as_accounts() {
        // Rate-limit accounting and "these two nodes are one mailbox" both read this, and a
        // node that claims an account it cannot use makes both answers wrong.
        let mut caps = caps();
        caps.add(mailbox("real@example.com", &[Role::Sink]).authenticated());
        caps.add(mailbox("unverified@example.com", &[Role::Sink]));

        let accounts: Vec<&AccountId> = caps.accounts().collect();
        assert_eq!(accounts, vec![&AccountId("real@example.com".into())]);
    }

    #[test]
    fn charging_battery_counts_as_mains() {
        assert!(PowerSource::Battery {
            percent: 40,
            charging: true
        }
        .on_mains());
        assert!(!PowerSource::Battery {
            percent: 90,
            charging: false
        }
        .on_mains());
    }

    #[test]
    fn the_account_derivation_is_pinned_to_a_known_answer() {
        // A format test, not a maths test. This value is compared across nodes, so a node on a
        // build that computes it differently does not fail — it silently matches nobody, and
        // per-account accounting quietly stops working. Change this deliberately, with a new
        // version in `ACCOUNT_SALT`.
        assert_eq!(
            AccountId::of_account("00000000-0000-4000-8000-000000000001").0,
            "acct:07680baad11285fc"
        );
    }

    #[test]
    fn the_same_account_fingerprints_the_same_however_it_was_written_down() {
        // Two nodes reading the same uuid out of two config files must agree, and trailing
        // whitespace from a hand-edited file is the realistic way they would not.
        assert_eq!(
            AccountId::of_account("00000000-0000-4000-8000-000000000001"),
            AccountId::of_account("  00000000-0000-4000-8000-000000000001\n")
        );
    }

    #[test]
    fn two_accounts_do_not_share_a_fingerprint() {
        assert_ne!(
            AccountId::of_account("00000000-0000-4000-8000-000000000001"),
            AccountId::of_account("9d2e7a01-3b8c-4f52-8e17-6c4b9a0d3f28")
        );
    }

    #[test]
    fn an_api_key_and_an_account_are_different_accounts_even_spelled_alike() {
        // The domain tag earning its place: without it, a key and a uuid with the same
        // characters would report as one account sharing one rate limit.
        assert_ne!(
            AccountId::of_api_key("00000000-0000-4000-8000-000000000001"),
            AccountId::of_account("00000000-0000-4000-8000-000000000001")
        );
    }

    #[test]
    fn two_api_keys_are_two_accounts() {
        // The bug this replaced: every node with any key set reported the literal string
        // "env:ANTHROPIC_API_KEY", so two unrelated accounts shared one cap.
        assert_ne!(
            AccountId::of_api_key("sk-ant-aaa"),
            AccountId::of_api_key("sk-ant-bbb")
        );
    }
}
