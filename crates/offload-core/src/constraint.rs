//! What a run *needs*, as a boolean tree over [`Capabilities`].
//!
//! Two entry points, and they must stay in step: [`Constraint::matches`] decides, and
//! [`Constraint::explain`] says why. "Twelve nodes match, why is my run pending" is the
//! single most common question this system has to answer, and an `explain` that has drifted
//! from `matches` is worse than none.

use crate::capability::{
    AccountId, AgentKind, Arch, Capabilities, DeviceClass, Os, Role, Service, Stability,
};
use crate::id::NodeId;
use crate::version;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Constraint {
    /// Matches anything. Identity element for `All`, and the default: a spec that states no
    /// preference prefers nothing.
    #[default]
    Always,
    All(Vec<Constraint>),
    Any(Vec<Constraint>),
    Not(Box<Constraint>),

    Os(Os),
    Arch(Arch),
    DeviceClass(DeviceClass),
    MinStability(Stability),

    MinCores(u32),
    MinMemoryMb(u64),
    MinDiskFreeMb(u64),
    OnMains,
    UnmeteredNetwork,

    /// Agent installed. Says nothing about auth — pair with `AgentAuthenticated`.
    HasAgent(AgentKind),
    AgentAuthenticated(AgentKind),
    /// Guards against migrating a run onto an older agent, whose resume format may not
    /// understand the transcript. See ADR-0003.
    MinAgentVersion(AgentKind, String),
    HasModel(AgentKind, String),

    HasToolchain(String),
    MinToolchainVersion(String, String),
    HasTag(String),
    Label(String, String),

    /// Any capability offering this service, optionally in a particular role.
    ///
    /// The general form the delivery plane needs (ADR-0010, ADR-0011): routing a notification
    /// is the same match-and-explain decision as placing a run, so it uses the same
    /// constraint tree rather than a second one that drifts.
    HasService {
        service: Service,
        role: Option<Role>,
    },
    /// ... and it actually works. Separate, because "has a mailbox configured" and "can
    /// actually send" fail for different reasons and deserve different messages.
    ///
    /// [`Capability::authenticated`] is one bit with a different meaning per role, and both are
    /// *verified rather than assumed*: for a sink it is that the credential works, and for
    /// [`Role::Execute`] it is that the nominated program is on the device **and can be run** —
    /// which ADR-0019 §1 calls the one thing a general-purpose machine can honestly verify. So
    /// this is the clause
    /// the **task** tier places on, and [`Constraint::HasService`] is not: that one asks whether
    /// anybody *nominated* a program, which is true of a `[[tasks]]` entry pointing at a path
    /// that does not exist. Measured: such a node bid, won its own round, and failed the run a
    /// millisecond later.
    ServiceAuthenticated {
        service: Service,
        role: Option<Role>,
    },
    /// ... acting as a specific account. "Send as *this* address", or "these two nodes are
    /// the same mailbox".
    ServiceIdentity {
        service: Service,
        identity: AccountId,
    },
    /// A resource offering this service that a run could actually be granted (ADR-0011).
    ///
    /// Three conditions in one variant rather than a conjunction the caller assembles, because
    /// getting any of them wrong is silent: it must play *some* [`Role::Resource`], it must be
    /// authenticated, and the access it offers is deliberately not part of the question — see
    /// [`crate::Capability::is_resource`].
    ///
    /// `HasService { role: Some(..) }` cannot express it. A role there is matched exactly, so a
    /// run asking for `Resource { access: ReadWrite }` would decline a node offering `Read` even
    /// where reading is all it wanted, and `role: None` is worse in the other direction: a phone
    /// with an *email sink* satisfies "has email", and a run that wanted to read a mailbox would
    /// be placed on a device that can only send to one.
    ///
    /// Placement, until a resource can be reached across the mesh. A run granted a resource is
    /// placed where the resource is, or refused to the operator's face (ADR-0014) — which is the
    /// honest behaviour rather than a temporary one, and the same shape as a local-path repo.
    CanUse {
        service: Service,
    },
    /// *This particular node* (ADR-0063 §3) — spelled `here` or `node=<name>` and resolved to an
    /// id when the run is submitted, never later.
    ///
    /// The one clause about identity rather than ability, which is why it is not answered from
    /// [`Capabilities`] alone: [`Constraint::matches_on`] asks the node *about itself*, and the
    /// identity-blind [`Constraint::matches`] answers **no** — an evaluation that does not know
    /// whose capabilities it is reading cannot claim they are the right machine's. Every
    /// placement reader has the id to hand and uses `matches_on`.
    ///
    /// Mostly a **preference** ([`crate::RunSpec::prefer`]), where it is scored and never
    /// filters; as a requirement it is a pin, which is `--require here` saying what it means.
    Node(NodeId),
}

impl Constraint {
    #[must_use]
    pub fn all(items: impl IntoIterator<Item = Constraint>) -> Constraint {
        Constraint::All(items.into_iter().collect())
    }

    #[must_use]
    pub fn any(items: impl IntoIterator<Item = Constraint>) -> Constraint {
        Constraint::Any(items.into_iter().collect())
    }

    /// The usual baseline for an agent run: the agent exists, is logged in, and is new
    /// enough to understand a transcript written by `min_version`.
    #[must_use]
    pub fn agent_ready(kind: AgentKind, min_version: Option<String>) -> Constraint {
        let mut parts = vec![
            Constraint::HasAgent(kind.clone()),
            Constraint::AgentAuthenticated(kind.clone()),
        ];
        if let Some(v) = min_version {
            parts.push(Constraint::MinAgentVersion(kind, v));
        }
        Constraint::All(parts)
    }

    /// Does a node with these capabilities, **identity unknown**, satisfy this?
    ///
    /// [`Constraint::Node`] answers `false` here, and `Not(Node(..))` therefore `true` — which is
    /// why no placement reader calls this: they all know whose capabilities they hold and ask
    /// [`Constraint::matches_on`]. What is left is the local `offload match` question and tests.
    #[must_use]
    pub fn matches(&self, caps: &Capabilities) -> bool {
        self.eval(None, caps)
    }

    /// Does *this node* satisfy this? The question a bidder asks about itself.
    #[must_use]
    pub fn matches_on(&self, node: NodeId, caps: &Capabilities) -> bool {
        self.eval(Some(node), caps)
    }

    /// Whether anything in the tree is a [`Constraint::Node`] clause, and so whether an
    /// identity-blind answer could be wrong.
    #[must_use]
    pub fn names_a_node(&self) -> bool {
        match self {
            Constraint::Node(_) => true,
            Constraint::All(items) | Constraint::Any(items) => {
                items.iter().any(Constraint::names_a_node)
            }
            Constraint::Not(inner) => inner.names_a_node(),
            _ => false,
        }
    }

    /// The nodes this tree names, in order, for a report that wants to say *which* machine a
    /// preference was for.
    #[must_use]
    pub fn named_nodes(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect_nodes(&mut out);
        out
    }

    fn collect_nodes(&self, out: &mut Vec<NodeId>) {
        match self {
            Constraint::Node(id) => {
                if !out.contains(id) {
                    out.push(*id);
                }
            }
            Constraint::All(items) | Constraint::Any(items) => {
                for item in items {
                    item.collect_nodes(out);
                }
            }
            Constraint::Not(inner) => inner.collect_nodes(out),
            _ => {}
        }
    }

    /// `self` and `other`, flattened where either is already a conjunction, and with `Always`
    /// dropped — so ANDing a preference into a requirement reads as one list in `explain`.
    #[must_use]
    pub fn and(self, other: Constraint) -> Constraint {
        let mut parts = Vec::new();
        for c in [self, other] {
            match c {
                Constraint::Always => {}
                Constraint::All(items) => parts.extend(items),
                other => parts.push(other),
            }
        }
        match parts.len() {
            0 => Constraint::Always,
            1 => parts.pop().unwrap_or(Constraint::Always),
            _ => Constraint::All(parts),
        }
    }

    fn eval(&self, node: Option<NodeId>, caps: &Capabilities) -> bool {
        match self {
            Constraint::Always => true,
            Constraint::All(items) => items.iter().all(|c| c.eval(node, caps)),
            Constraint::Any(items) => items.iter().any(|c| c.eval(node, caps)),
            Constraint::Not(inner) => !inner.eval(node, caps),
            Constraint::Node(id) => node == Some(*id),

            Constraint::Os(os) => &caps.os == os,
            Constraint::Arch(arch) => &caps.arch == arch,
            Constraint::DeviceClass(class) => caps.device_class == *class,
            Constraint::MinStability(min) => caps.stability >= *min,

            Constraint::MinCores(n) => caps.cpu_cores >= *n,
            Constraint::MinMemoryMb(mb) => caps.memory_mb >= *mb,
            Constraint::MinDiskFreeMb(mb) => caps.disk_free_mb >= *mb,
            Constraint::OnMains => caps.power.on_mains(),
            // A run asking for this is asking for a *guarantee* that its bytes are free, and
            // `Unknown` is not one — so a node nobody could ask does not match (ADR-0045 §4).
            // This is the one reader whose behaviour changes: it used to match every node in
            // the fleet, because the probe reported `false` everywhere.
            Constraint::UnmeteredNetwork => caps.metered_network.known_unmetered(),

            Constraint::HasAgent(kind) => caps.agent(kind).is_some(),
            Constraint::AgentAuthenticated(kind) => {
                caps.agent(kind).is_some_and(|a| a.authenticated)
            }
            Constraint::MinAgentVersion(kind, want) => caps
                .agent_details(kind)
                .is_some_and(|a| version::at_least(&a.version, want)),
            Constraint::HasModel(kind, model) => {
                caps.agent_details(kind).is_some_and(|a| a.reaches(model))
            }

            Constraint::HasService { service, role } => caps
                .offering(service)
                .any(|c| role.is_none_or(|role| c.plays(role))),
            Constraint::ServiceAuthenticated { service, role } => caps
                .offering(service)
                .any(|c| c.authenticated && role.is_none_or(|role| c.plays(role))),
            Constraint::ServiceIdentity { service, identity } => caps
                .offering(service)
                .any(|c| c.identity.as_ref() == Some(identity)),

            Constraint::CanUse { service } => caps
                .offering(service)
                .any(|c| c.is_resource() && c.authenticated),

            Constraint::HasToolchain(name) => caps.toolchains.contains_key(name),
            Constraint::MinToolchainVersion(name, want) => caps
                .toolchains
                .get(name)
                .is_some_and(|have| version::at_least(have, want)),
            Constraint::HasTag(tag) => caps.tags.contains(tag),
            Constraint::Label(key, value) => caps.labels.get(key).is_some_and(|v| v == value),
        }
    }

    /// Structured account of which clauses held and which did not, identity unknown — see
    /// [`Constraint::matches`] for what that means for a [`Constraint::Node`] clause.
    #[must_use]
    pub fn explain(&self, caps: &Capabilities) -> Explain {
        self.explain_with(None, caps, &|id: NodeId| id.short())
    }

    /// [`Constraint::explain`] for a node that knows who it is. In step with
    /// [`Constraint::matches_on`] for the reason the module comment gives.
    #[must_use]
    pub fn explain_on(&self, node: NodeId, caps: &Capabilities) -> Explain {
        self.explain_with(Some(node), caps, &|id: NodeId| id.short())
    }

    /// [`Constraint::explain_on`], with nodes named by `name` rather than by id. This crate has
    /// no names; the bid round has the view, and a refusal read at a keyboard said `is node
    /// 2a3b5d5d (this is 29be31be)` on a row already headed `alpha`, about `bravo`.
    #[must_use]
    pub fn explain_on_naming(
        &self,
        node: NodeId,
        caps: &Capabilities,
        name: &dyn Fn(NodeId) -> String,
    ) -> Explain {
        self.explain_with(Some(node), caps, name)
    }

    fn explain_with(
        &self,
        node: Option<NodeId>,
        caps: &Capabilities,
        name: &dyn Fn(NodeId) -> String,
    ) -> Explain {
        let satisfied = self.eval(node, caps);
        let children = match self {
            Constraint::All(items) | Constraint::Any(items) => items
                .iter()
                .map(|c| c.explain_with(node, caps, name))
                .collect(),
            Constraint::Not(inner) => vec![inner.explain_with(node, caps, name)],
            _ => Vec::new(),
        };
        Explain {
            label: match self {
                Constraint::Node(id) => format!("is node {}", name(*id)),
                _ => self.label(),
            },
            detail: match (self, node) {
                (Constraint::Node(_), Some(me)) => Some(format!("this is {}", name(me))),
                (Constraint::Node(_), None) => {
                    Some("asked without knowing which node this is".into())
                }
                _ => self.observed(caps),
            },
            satisfied,
            children,
        }
    }

    fn label(&self) -> String {
        match self {
            Constraint::Always => "always".into(),
            Constraint::All(items) => format!("all of {} clauses", items.len()),
            Constraint::Any(items) => format!("any of {} clauses", items.len()),
            Constraint::Not(_) => "not".into(),
            Constraint::Os(os) => format!("os == {os}"),
            Constraint::Arch(arch) => format!("arch == {arch}"),
            Constraint::DeviceClass(c) => format!("device class == {c}"),
            Constraint::MinStability(s) => format!("stability >= {s}"),
            Constraint::MinCores(n) => format!("cores >= {n}"),
            Constraint::MinMemoryMb(mb) => format!("memory >= {mb} MB"),
            Constraint::MinDiskFreeMb(mb) => format!("free disk >= {mb} MB"),
            Constraint::OnMains => "on mains power".into(),
            Constraint::UnmeteredNetwork => "unmetered network".into(),
            Constraint::HasService { service, role } => match role {
                Some(role) => format!("has {service} as {role}"),
                None => format!("has {service}"),
            },
            Constraint::ServiceAuthenticated { service, role } => match role {
                // "authenticated" is the word for a credential, and under `Execute` the same
                // bit means *this device can run the program its owner nominated*. An operator
                // reading why their task would not place must not be sent looking for a login.
                //
                // Two cuts of this were wrong before the walk agreed with it. `runs {service},
                // program present` stated the opposite of what had been found, one word after
                // `ineligible:` — a *requirement* printed as though it were the observation.
                // `with its program there` then asserted the cause, and there are two: the
                // program may be absent, or present and not executable. Core cannot tell them
                // apart — it has the bit and not the filesystem — so it says neither, and the
                // node-local reports (`offload status`, `offload probe`, the submission's own
                // refusal) are where the cause is named.
                Some(Role::Execute) => format!("has {service} as execute, and can run it"),
                Some(role) => format!("has {service} as {role}, authenticated"),
                None => format!("has {service}, authenticated"),
            },
            Constraint::ServiceIdentity { service, identity } => {
                format!("has {service} acting as {}", identity.0)
            }
            Constraint::CanUse { service } => {
                format!("offers {service} for a run to use, working")
            }
            Constraint::HasAgent(k) => format!("has agent {k}"),
            Constraint::AgentAuthenticated(k) => format!("{k} authenticated"),
            Constraint::MinAgentVersion(k, v) => format!("{k} >= {v}"),
            Constraint::HasModel(k, m) => format!("{k} can reach model {m}"),
            Constraint::HasToolchain(t) => format!("has toolchain {t}"),
            Constraint::MinToolchainVersion(t, v) => format!("toolchain {t} >= {v}"),
            Constraint::HasTag(t) => format!("has tag {t}"),
            Constraint::Label(k, v) => format!("label {k} == {v}"),
            Constraint::Node(id) => format!("is node {}", id.short()),
        }
    }

    /// What the node actually reported, so a failure reads
    /// `cores >= 16 (have 8)` rather than just `cores >= 16`.
    fn observed(&self, caps: &Capabilities) -> Option<String> {
        match self {
            Constraint::Os(_) => Some(format!("have {}", caps.os)),
            Constraint::Arch(_) => Some(format!("have {}", caps.arch)),
            Constraint::DeviceClass(_) => Some(format!("have {}", caps.device_class)),
            Constraint::MinStability(_) => Some(format!("have {}", caps.stability)),
            Constraint::MinCores(_) => Some(format!("have {}", caps.cpu_cores)),
            Constraint::MinMemoryMb(_) => Some(format!("have {} MB", caps.memory_mb)),
            Constraint::MinDiskFreeMb(_) => Some(format!("have {} MB", caps.disk_free_mb)),
            Constraint::OnMains => Some(format!("have {}", caps.power)),
            Constraint::UnmeteredNetwork => Some(match caps.metered_network {
                crate::Metered::Unknown => {
                    "metered = unknown, and this run needs it known to be free".to_string()
                }
                known => format!("metered = {known}"),
            }),
            Constraint::HasAgent(k) | Constraint::AgentAuthenticated(k) => {
                Some(match (caps.agent(k), caps.agent_details(k)) {
                    (Some(cap), Some(details)) => format!(
                        "installed {}, authenticated = {}",
                        details.version, cap.authenticated
                    ),
                    _ => "not installed".into(),
                })
            }
            Constraint::MinAgentVersion(k, _) => Some(match caps.agent_details(k) {
                Some(a) => format!("have {}", a.version),
                None => "not installed".into(),
            }),
            Constraint::HasModel(k, _) => Some(match caps.agent_details(k) {
                Some(a) => format!("reachable: {}", join(a.models.iter().map(|m| &m.value))),
                None => "not installed".into(),
            }),
            Constraint::HasService { service, .. }
            | Constraint::ServiceAuthenticated { service, .. }
            | Constraint::ServiceIdentity { service, .. }
            | Constraint::CanUse { service } => {
                let found: Vec<String> = caps
                    .offering(service)
                    .map(|c| {
                        format!(
                            "{} ({}{})",
                            c.id,
                            join_owned(c.roles.iter().map(ToString::to_string)),
                            match (c.authenticated, c.plays(Role::Execute)) {
                                (true, _) => "",
                                // The same bit, the other meaning. `task:nightly (execute,
                                // unauthenticated)` sends somebody to look for a login for a
                                // shell script. Which of the two reasons it is belongs to the
                                // node that measured it and not to this function — see the
                                // label arm above.
                                (false, true) => ", this node cannot run it",
                                (false, false) => ", unauthenticated",
                            }
                        )
                    })
                    .collect();
                Some(if found.is_empty() {
                    "nothing offers it".into()
                } else {
                    found.join("; ")
                })
            }
            Constraint::HasToolchain(t) | Constraint::MinToolchainVersion(t, _) => {
                Some(match caps.toolchains.get(t) {
                    Some(v) => format!("have {v}"),
                    None => "not installed".into(),
                })
            }
            Constraint::HasTag(_) => Some(format!("tags: {}", join(caps.tags.iter()))),
            Constraint::Label(k, _) => Some(match caps.labels.get(k) {
                Some(v) => format!("have {v}"),
                None => "unset".into(),
            }),
            // Written out rather than `_ => None`, and this is not tidiness. The catch-all is
            // what let `CanUse` ship explaining the *requirement* and never what the node has —
            // which is the half of `explain` worth having, since "why is my run pending" is
            // answered by "nothing offers it" and not by repeating what was asked for. A new
            // variant now fails to compile here, and deciding it has nothing to observe is a
            // decision somebody makes rather than one they inherit.
            //
            // These genuinely have nothing: the structural ones describe their children, and
            // `Always` describes nothing at all.
            //
            // `Node` observes an identity, not a capability, and `explain_with` fills it in from
            // the id it was asked with — which this function does not have.
            Constraint::Always
            | Constraint::All(_)
            | Constraint::Any(_)
            | Constraint::Not(_)
            | Constraint::Node(_) => None,
        }
    }
}

fn join_owned(items: impl Iterator<Item = String>) -> String {
    let joined: Vec<String> = items.collect();
    if joined.is_empty() {
        "none".into()
    } else {
        joined.join(", ")
    }
}

fn join<'a>(items: impl Iterator<Item = &'a String>) -> String {
    let joined: Vec<&str> = items.map(String::as_str).collect();
    if joined.is_empty() {
        "none".into()
    } else {
        joined.join(", ")
    }
}

/// A rendered account of one constraint evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explain {
    pub label: String,
    pub detail: Option<String>,
    pub satisfied: bool,
    pub children: Vec<Explain>,
}

impl Explain {
    /// The leaf clauses that failed — what an operator actually wants printed.
    ///
    /// Descends only through unsatisfied branches, so an `Any` that succeeded does not
    /// report its losing arms as problems.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_failures(&mut out);
        out
    }

    fn collect_failures(&self, out: &mut Vec<String>) {
        if self.satisfied {
            return;
        }
        if self.children.is_empty() {
            out.push(match &self.detail {
                Some(d) => format!("{} ({d})", self.label),
                None => self.label.clone(),
            });
        } else {
            let mine = out.len();
            for child in &self.children {
                child.collect_failures(out);
            }
            // An `Any` whose arms all failed reports its arms, which is right. A `Not`
            // whose inner clause succeeded has no failing leaf, so say so explicitly.
            //
            // Measured against *this* node's own children, not against the whole list. It used
            // to ask whether anything at all had been collected, which made the answer depend
            // on where the clause sat: `All([Not(Always), MinCores(99)])` reported both, and
            // `All([MinCores(99), Not(Always)])` dropped the `Not`, because a sibling had
            // already filled the vector. Two spellings of one requirement, two accounts of why
            // it was not met — and the constraint list is assembled from a repo config, a
            // `--constraint` string and `Constraint::agent_ready`, so nobody chose the order.
            if out.len() == mine {
                out.push(self.label.clone());
            }
        }
    }

    /// Indented tree for `offload match -v`.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.render_into(&mut out, 0);
        out
    }

    fn render_into(&self, out: &mut String, depth: usize) {
        let mark = if self.satisfied { "ok  " } else { "FAIL" };
        let pad = "  ".repeat(depth);
        out.push_str(&format!("{pad}{mark} {}", self.label));
        if let Some(d) = &self.detail {
            out.push_str(&format!("  [{d}]"));
        }
        out.push('\n');
        for child in &self.children {
            child.render_into(out, depth + 1);
        }
    }
}

/// A node named in an expression before it has been looked up (ADR-0063 §3).
///
/// Never stored in a spec: [`Wanted::resolve`] turns each into a [`Constraint::Node`] at
/// submission, because *here* typed on the laptop has to go on meaning the laptop after the run
/// has been handed to the desktop, and a name is a node's own label where an id is what the fleet
/// agrees on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRef {
    /// The daemon that takes the submission — not wherever the submitter is later, which
    /// nothing observes.
    Here,
    /// A member's name, matched exactly against the names in the view.
    Named(String),
}

impl std::fmt::Display for NodeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeRef::Here => f.write_str("here"),
            NodeRef::Named(name) => write!(f, "node={name}"),
        }
    }
}

/// An expression as typed: the clauses that are about capabilities, and the nodes it names.
///
/// The CLI's grammar is a comma-separated conjunction, so the node clauses can be carried beside
/// the rest rather than inside the tree — the CLI does not know the fleet's ids and must not
/// guess them. A rule stores the **resolved** form (`nodes` empty), so that a `here` typed when
/// the rule was written keeps meaning that machine at every firing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Wanted {
    #[serde(default = "always")]
    pub clauses: Constraint,
    #[serde(default)]
    pub nodes: Vec<NodeRef>,
}

fn always() -> Constraint {
    Constraint::Always
}

/// Why a node named in an expression could not be looked up. Refused at submission, because a
/// preference for a machine nobody can identify is one that would silently never be met.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unresolved {
    #[error("no member of this fleet is called `{0}`")]
    Unknown(String),
    #[error("`{name}` is the name of {} members ({}) — name one by a name only it has",
        ids.len(), ids.iter().map(NodeId::short).collect::<Vec<_>>().join(", "))]
    Ambiguous { name: String, ids: Vec<NodeId> },
}

impl Wanted {
    /// A constraint that names no node.
    #[must_use]
    pub fn of(clauses: Constraint) -> Wanted {
        Wanted {
            clauses,
            nodes: Vec::new(),
        }
    }

    /// Every node reference looked up and ANDed in: `here` is `me`, a name must belong to exactly
    /// one of `members`.
    ///
    /// # Errors
    ///
    /// A name nobody has, or one that more than one member has.
    pub fn resolve(
        &self,
        me: NodeId,
        members: &[(NodeId, String)],
    ) -> Result<Constraint, Unresolved> {
        let mut out = self.clauses.clone();
        for node in &self.nodes {
            let id = match node {
                NodeRef::Here => me,
                NodeRef::Named(name) => {
                    let ids: Vec<NodeId> = members
                        .iter()
                        .filter(|(_, n)| n == name)
                        .map(|(id, _)| *id)
                        .collect();
                    match ids.as_slice() {
                        [] => return Err(Unresolved::Unknown(name.clone())),
                        [id] => *id,
                        _ => {
                            return Err(Unresolved::Ambiguous {
                                name: name.clone(),
                                ids,
                            })
                        }
                    }
                }
            };
            out = out.and(Constraint::Node(id));
        }
        Ok(out)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clauses == Constraint::Always && self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{AgentDetails, Capability, PowerSource};

    /// The difference between the two service clauses, which is the whole of the task tier's
    /// placement question (ADR-0019 §1).
    ///
    /// `authenticated` on a `Role::Execute` capability means *the nominated program is on this
    /// device*, and `HasService` cannot see it — so a `[[tasks]]` entry pointing at a path that
    /// does not exist satisfied the constraint a task was placed on. Measured on one daemon:
    /// accepted, placed here, failed a millisecond later, then restarted by the recovery tick
    /// until its budget went.
    #[test]
    fn a_task_places_on_a_program_that_is_there_and_not_merely_on_one_that_was_nominated() {
        let service = Service::Other("nightly".into());
        let mut node = Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop);
        let mut nominated = Capability::new("task:nightly", service.clone(), [Role::Execute]);
        nominated.description = "posts the nightly report".into();
        nominated.unusable("its program is not on this device");
        node.add(nominated);

        let nominates = Constraint::HasService {
            service: service.clone(),
            role: Some(Role::Execute),
        };
        let can_run = Constraint::ServiceAuthenticated {
            service: service.clone(),
            role: Some(Role::Execute),
        };
        assert!(
            nominates.matches(&node),
            "the weaker clause is why this was a defect: somebody did nominate it"
        );
        assert!(
            !can_run.matches(&node),
            "and nobody on this device can run it"
        );

        // The same node with the program there: both hold, so the fix refuses nothing that
        // used to work.
        let mut working = Capability::new("task:nightly", service.clone(), [Role::Execute]);
        working.authenticated = true;
        node.add(working);
        assert!(nominates.matches(&node));
        assert!(can_run.matches(&node));
    }

    /// The sentence an operator reads when a task will not place must not be about a login.
    #[test]
    fn the_execute_role_explains_itself_as_a_program_rather_than_a_credential() {
        let service = Service::Other("nightly".into());
        let execute = Constraint::ServiceAuthenticated {
            service: service.clone(),
            role: Some(Role::Execute),
        }
        .explain(&Capabilities::empty(
            Os::Linux,
            Arch::X86_64,
            DeviceClass::Laptop,
        ));
        assert!(
            !execute.label.contains("authenticated"),
            "a nominated program has no credential: {}",
            execute.label
        );
        assert!(execute.label.contains("nightly"), "{}", execute.label);

        // …and the roles that *do* have one keep the word.
        let sink = Constraint::ServiceAuthenticated {
            service: Service::Push,
            role: Some(Role::Sink),
        }
        .explain(&Capabilities::empty(
            Os::Linux,
            Arch::X86_64,
            DeviceClass::Laptop,
        ));
        assert!(sink.label.contains("authenticated"), "{}", sink.label);
    }

    /// A `Node` clause is about identity, so an evaluation that does not know whose
    /// capabilities it holds must not claim they are the right machine's — and `explain` agrees
    /// with `matches` in both forms, which is the module's one rule.
    #[test]
    fn a_node_clause_is_answered_only_by_a_node_that_knows_who_it_is() {
        let me = NodeId::from_bytes([7; 32]);
        let other = NodeId::from_bytes([8; 32]);
        let caps = laptop();
        let here = Constraint::Node(me);
        assert!(here.matches_on(me, &caps));
        assert!(!here.matches_on(other, &caps));
        assert!(!here.matches(&caps), "unknown identity is not a match");
        assert!(here.explain_on(me, &caps).satisfied);
        assert!(!here.explain_on(other, &caps).satisfied);
        assert!(!here.explain(&caps).satisfied);
        assert!(!here.explain_on(other, &caps).failures().is_empty());
        // …and a value in the spelling the grammar takes, not the enum's.
        let mac = Constraint::Os(Os::MacOs).explain(&caps).failures();
        assert_eq!(mac, vec!["os == macos (have linux)".to_string()], "{mac:?}");
        let named = |id: NodeId| {
            if id == other {
                "bravo".to_string()
            } else {
                id.short()
            }
        };
        let said = here.explain_on_naming(other, &caps, &named).failures();
        assert!(
            said.iter().any(|f| f.contains("(this is bravo)")),
            "a node refusing a pin names itself the way the reader does: {said:?}"
        );
    }

    #[test]
    fn a_named_node_resolves_to_exactly_one_member_or_is_refused() {
        let me = NodeId::from_bytes([1; 32]);
        let desktop = NodeId::from_bytes([2; 32]);
        let twin = NodeId::from_bytes([3; 32]);
        let members = vec![(me, "laptop".to_string()), (desktop, "desktop".to_string())];
        let wanted = |nodes| Wanted {
            clauses: Constraint::MinCores(4),
            nodes,
        };
        assert_eq!(
            wanted(vec![NodeRef::Here]).resolve(me, &members),
            Ok(Constraint::All(vec![
                Constraint::MinCores(4),
                Constraint::Node(me)
            ]))
        );
        assert_eq!(
            wanted(vec![NodeRef::Named("desktop".into())]).resolve(me, &members),
            Ok(Constraint::All(vec![
                Constraint::MinCores(4),
                Constraint::Node(desktop)
            ]))
        );
        assert_eq!(
            wanted(vec![NodeRef::Named("phone".into())]).resolve(me, &members),
            Err(Unresolved::Unknown("phone".into()))
        );
        let mut two = members.clone();
        two.push((twin, "desktop".to_string()));
        assert!(matches!(
            wanted(vec![NodeRef::Named("desktop".into())]).resolve(me, &two),
            Err(Unresolved::Ambiguous { ids, .. }) if ids.len() == 2
        ));
        // Nothing named, nothing added: a submission with no `--prefer` stays `Always`.
        assert_eq!(
            Wanted::default().resolve(me, &members),
            Ok(Constraint::Always)
        );
    }

    fn laptop() -> Capabilities {
        let mut caps = Capabilities::empty(Os::Linux, Arch::X86_64, DeviceClass::Laptop);
        caps.cpu_cores = 8;
        caps.memory_mb = 16_384;
        caps.disk_free_mb = 200_000;
        caps.power = PowerSource::Battery {
            percent: 55,
            charging: false,
        };
        caps.add(Capability::agent(
            AgentKind::ClaudeCode,
            AgentDetails {
                version: "2.10.0".into(),
                models: vec![crate::capability::Model::named("claude-opus-5")],
                max_concurrent: 2,
            },
            true,
        ));
        caps.toolchains.insert("rust".into(), "1.82.0".into());
        caps
    }

    #[test]
    fn a_service_constraint_matches_by_role_and_explains_what_was_there() {
        // The reason for the whole capability reshape: routing a notification has to be the
        // same match-and-explain decision as placing a run (ADR-0010, ADR-0011).
        let mut caps = laptop();
        caps.add(
            Capability::new(
                "email:me@example.com",
                Service::Email,
                [crate::capability::Role::Sink],
            )
            .with_identity(Some(AccountId("me@example.com".into())))
            .authenticated(),
        );

        let sink = Constraint::HasService {
            service: Service::Email,
            role: Some(crate::capability::Role::Sink),
        };
        assert!(sink.matches(&caps));

        // The same mailbox is not a thing a run may read just because it can be sent to.
        let readable = Constraint::HasService {
            service: Service::Email,
            role: Some(crate::capability::Role::Resource {
                access: crate::capability::Access::Read,
            }),
        };
        assert!(!readable.matches(&caps));

        let explained = readable.explain(&caps);
        assert!(!explained.satisfied);
        // The refusal names what the node actually has, which is the difference between
        // "why is my run pending" being answerable and not.
        assert!(explained.failures()[0].contains("sink"));
    }

    #[test]
    fn a_run_is_only_placed_where_a_resource_it_was_granted_actually_works() {
        use crate::capability::{Access, Role};

        // A phone with a mailbox it can *send to*. `HasService { role: None }` would place a run
        // that wanted to read mail right here, on a device that can only shout into it.
        let mut sink_only = laptop();
        sink_only.add(Capability::new("sink:mail", Service::Email, [Role::Sink]).authenticated());
        let can_use = Constraint::CanUse {
            service: Service::Email,
        };
        assert!(!can_use.matches(&sink_only), "a sink is not a resource");

        // A resource whose program is missing is advertised unauthenticated, which is what makes
        // "configured but broken" explainable instead of invisible — and it must not win a bid.
        let mut broken = laptop();
        broken.add(Capability::new(
            "resource:mail",
            Service::Email,
            [Role::Resource {
                access: Access::Read,
            }],
        ));
        assert!(!can_use.matches(&broken));
        // And the refusal says what is *there*, not just what was wanted. "why is my run
        // pending" is answered by "it is configured and its program is missing", and a line
        // that only repeated the requirement would answer nothing.
        let failure = &can_use.explain(&broken).failures()[0];
        assert!(failure.contains("email"), "{failure}");
        assert!(
            failure.contains("unauthenticated"),
            "the refusal must name what the node actually has: {failure}"
        );

        // And the access it offers is not part of the question. The owner knows whether their
        // integration reads or writes; the person submitting a run knows only that it needs
        // email, so a run asking for one access and finding the other would refuse to place
        // beside a resource that would have done.
        for access in [Access::Read, Access::Write, Access::ReadWrite] {
            let mut caps = laptop();
            caps.add(
                Capability::new("resource:mail", Service::Email, [Role::Resource { access }])
                    .authenticated(),
            );
            assert!(can_use.matches(&caps), "{access:?} should place");
        }
    }

    #[test]
    fn a_service_nobody_offers_says_so_rather_than_going_quiet() {
        let constraint = Constraint::HasService {
            service: Service::Push,
            role: None,
        };
        let explained = constraint.explain(&laptop());
        assert!(!explained.satisfied);
        assert!(explained.failures()[0].contains("nothing offers it"));
    }

    #[test]
    fn an_identity_constraint_distinguishes_two_nodes_holding_the_same_service() {
        // Two nodes claiming `email` are not interchangeable if they send as different
        // addresses, and not independent if they send as the same one.
        let mut caps = laptop();
        caps.add(
            Capability::new(
                "email:work",
                Service::Email,
                [crate::capability::Role::Sink],
            )
            .with_identity(Some(AccountId("work@example.com".into())))
            .authenticated(),
        );

        assert!(Constraint::ServiceIdentity {
            service: Service::Email,
            identity: AccountId("work@example.com".into()),
        }
        .matches(&caps));
        assert!(!Constraint::ServiceIdentity {
            service: Service::Email,
            identity: AccountId("home@example.com".into()),
        }
        .matches(&caps));
    }

    #[test]
    fn agent_ready_matches_an_installed_authed_agent() {
        let c = Constraint::agent_ready(AgentKind::ClaudeCode, Some("2.9.0".into()));
        assert!(c.matches(&laptop()));
    }

    #[test]
    fn version_gate_uses_numeric_comparison() {
        // Lexically "2.10.0" < "2.9.0"; numerically it is newer. Guards the migration
        // eligibility rule in ADR-0003.
        let c = Constraint::MinAgentVersion(AgentKind::ClaudeCode, "2.9.0".into());
        assert!(c.matches(&laptop()));
    }

    #[test]
    fn failures_name_the_clause_and_the_observation() {
        let c = Constraint::all([
            Constraint::MinCores(16),
            Constraint::OnMains,
            Constraint::HasToolchain("rust".into()),
        ]);
        let explain = c.explain(&laptop());
        assert!(!explain.satisfied);

        let failures = explain.failures();
        assert_eq!(
            failures.len(),
            2,
            "only two clauses should fail: {failures:?}"
        );
        assert!(failures[0].contains("cores >= 16") && failures[0].contains("have 8"));
        assert!(failures[1].contains("mains"));
    }

    #[test]
    fn satisfied_any_does_not_report_its_losing_arms() {
        let c = Constraint::any([Constraint::MinCores(64), Constraint::MinCores(4)]);
        let explain = c.explain(&laptop());
        assert!(explain.satisfied);
        assert!(explain.failures().is_empty());
    }

    /// Found by `tests/properties.rs`: two spellings of one requirement, two answers.
    ///
    /// A branch whose children all held while the branch itself did not — a `Not` — has no
    /// failing leaf to report, so it reports itself. The check for that asked whether *anything*
    /// had been collected rather than anything of its own, so a sibling that failed first
    /// silenced it. The clause list comes from a repo config, a `--constraint` string or
    /// `Constraint::agent_ready`; nobody chose the order, so nobody could have chosen this.
    #[test]
    fn a_clause_is_explained_wherever_it_sits_in_the_list() {
        let unmeetable = Constraint::Not(Box::new(Constraint::Always));
        let also_unmeetable = Constraint::MinCores(64);
        let forward = Constraint::all([unmeetable.clone(), also_unmeetable.clone()]);
        let backward = Constraint::all([also_unmeetable, unmeetable]);

        let mut a = forward.explain(&laptop()).failures();
        let mut b = backward.explain(&laptop()).failures();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        assert_eq!(a.len(), 2, "both clauses are named: {a:?}");
    }

    #[test]
    fn explain_agrees_with_matches() {
        let caps = laptop();
        for c in [
            Constraint::Always,
            Constraint::MinCores(8),
            Constraint::MinCores(9),
            Constraint::Not(Box::new(Constraint::OnMains)),
            Constraint::agent_ready(AgentKind::ClaudeCode, None),
            Constraint::HasModel(AgentKind::ClaudeCode, "nope".into()),
        ] {
            assert_eq!(
                c.matches(&caps),
                c.explain(&caps).satisfied,
                "explain drifted from matches for {c:?}"
            );
        }
    }
}
