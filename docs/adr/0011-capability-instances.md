# ADR-0011: Capabilities are instances with roles and an identity; agent-facing prose is not one of their fields

**Status:** accepted · 2026-07-26 · refines the capability model of ADR-0005 · built in three
pieces: instances and roles 2026-07-27, `Role::Sink` with the delivery plane 2026-08-20,
`Role::Resource` 2026-08-21 · **amended 2026-08-21** three times below, where building
corrected it

## Context

`Capabilities` grew up serving one question: *can this device run this agent?* It answers that
well. `AgentCapability` is already the right shape for a self-describing thing — version,
`authenticated`, an `AccountId` for the account it acts as, the models it can actually reach.

Two pressures break the rest of the model.

**ADR-0010 made reporting a capability.** A delivery route needs everything an agent
capability needs: verification, an identity, per-service detail. If sinks arrive as `tags`
— a `BTreeSet<String>` — they arrive as strings that cannot say whether the credential
works or which mailbox they mean, and the delivery plane inherits a routing decision it
cannot explain.

**The same service appears in several roles.** Email is the clarifying example. "Can send
as me@example.com" and "can read everything arriving at me@example.com" are the same
integration, the same account, and two completely different grants. A capability vocabulary
with one axis has to spell them as two unrelated names and hope nobody confuses them.

There is also a structural limit: `agents: BTreeMap<AgentKind, AgentCapability>` allows one
entry per kind. Two mail accounts, or three chat workspaces, cannot be expressed at all.

And a question that looks like a field and is not. A capability wants a description "so an
agent can understand what it is" — but the scheduler and the model are different consumers
wanting different documents. One typed and small, matched and explained; one prose, tuned
by iteration, and only ever seen by a run that was granted the thing.

## Decision

**Capabilities are a set of instances, each with a stable id**, replacing the map keyed by
kind. Two mailboxes are two entries. Ownership is unchanged from ADR-0005: a node is
authoritative for its own capabilities, arbitrated by `incarnation`.

```rust
pub struct Capability {
    /// Stable on this node: "email:me@example.com", "agent:claude-code".
    pub id: CapabilityId,
    pub service: Service,             // closed enum, Other(String) escape
    pub roles: BTreeSet<Role>,
    /// Who it acts as. Comparable across nodes.
    pub identity: Option<AccountId>,
    /// Verified. Never assumed.
    pub authenticated: bool,
    /// Per-service payload: agent version and models, a mailbox's filters, ...
    pub details: ServiceDetails,
    /// One line, for `offload probe` and refusal messages. Never matched on.
    pub description: String,
}
```

**Three roles, because they are three different grants:**

- **`Sink`** — the fleet reaches a human through it. Fire and forget. ADR-0010.
- **`Trigger`** — something arrives and creates or wakes work: a webhook, mail matching a
  filter, a schedule firing.
- **`Resource { access }`** — a run acts on it while executing: reads an inbox, writes a
  calendar. Tool-shaped, and the only role that hands a run something.

**Identity is a first-class, comparable key.** `AccountId` stops being an agent detail. It
answers two questions with one field: *route this to me* (deliver as the address I asked
for), and *these two nodes are one resource* — the same reasoning that makes three nodes
authenticated as one Claude account share one rate limit. Two nodes claiming `email` are
not interchangeable if they send as different addresses, and are not independent if they
send as the same one.

**Only typed fields are matchable.** `service`, `roles`, `identity` and `authenticated`
participate in `Constraint`; `description` renders for humans and nothing else. A free-text
field that decisions are made against is `explain` drifting from `matches` permanently, and
that drift is the failure the constraint design exists to prevent.

**No model-facing prose on the wire.** The agent-facing description of a capability lives
with the adapter that implements it, not in the gossiped record, for four reasons: ADR-0005
gossips digests and fetches on change, and prose per capability per node is exactly the
payload that path should not carry; prose has no arbitration rule when two nodes describe
one account differently; model-facing text is iterated at a rate no wire format can follow;
and anything present in a gossiped struct is eventually matched on.

**Capabilities project into per-run tool definitions.** A `Resource` a run has been granted
is exposed to it as a tool, in the agent's own protocol — for Claude Code, an MCP config
generated at spawn time and passed with `--mcp-config`, alongside **`--strict-mcp-config`**
so the run sees exactly what Offload granted and *not* whatever MCP servers the host user
has configured for themselves. Both flags verified against 2.1.220. This is the same rule
the tool allowlist already enforces: nothing ambient, and a grant is a decision somebody
made rather than an accident of which machine won the bid.

**Using a resource is a grant, not a consequence of placement.** A node holding a mailbox
capability does not mean every run placed there may read mail. Grants are per run and
layered exactly like `ToolAllowlist`.

**A remote resource is a proxied call, never a shipped credential.** If the phone holds the
mailbox and the desktop hosts the agent, the desktop exposes the tool and forwards the call
to the phone. Credentials do not migrate (ADR-0002, ADR-0010); moving the call is the only
correct direction.

## Consequences

Good:

- One vocabulary covers agents, sinks, triggers and resources, so routing a notification is
  the same constraint-match-plus-policy decision as placing a run — one scheduler, one set
  of refusal reasons, one place `explain` has to stay honest.
- A device with no agent is a first-class member if it holds a sink or a trigger.
- The account-sharing problem gets one answer instead of two. Whatever solves per-account
  rate limits for agents solves "these two nodes are one mailbox".
- Model-facing text can be rewritten freely, because it was never a wire format.

Bad, and worth being clear-eyed about:

- **`Resource` forces a request/response path across the mesh**, with timeouts, failure
  semantics and backpressure — where a `Sink` is fire-and-forget. This is a substantial
  piece of work hiding behind a small-looking role, and it does not exist until phase 3
  gives it a transport.
- **The capability vocabulary is now open enough to be abused.** `Other(String)` is
  necessary and is also how matching degrades into string soup. Closed enums with an escape
  hatch keep the common path typed, but nothing stops a fleet from living in the escape
  hatch.
- **This is a breaking change to `Capabilities`**, which is why it is being written before
  phase 3 rather than after: today it is a struct edit, afterwards it is a protocol version.
- **It edges toward being an integration platform.** The line held here is that Offload
  *routes* capabilities and *projects* them into a run's tool set; it does not implement
  services. The moment an adapter starts parsing an inbox rather than handing the run a
  tool that reads one, this ADR has been violated.

## Alternatives

**Keep tags for everything non-agent.** Cheapest. Rejected: a tag cannot say whether the
credential works, which account it means, or which of three roles it plays — and a routing
decision made on a tag cannot explain itself.

**One description field serving both the scheduler and the model.** What prompted the
question. Rejected on the two-consumers argument: they want different documents, at
different sizes, changing at different rates, and merging them puts prompt text on the
gossip path.

**Offload defines its own tool-description format.** Rejected — the agent already has one,
and inventing a second is the "never reimplement an agent" line with the words changed.

**Ship credentials to whichever node runs the agent, so resources are always local.** Much
simpler, and rejected outright: it is the one thing ADR-0002 says to stop and reconsider the
design over.

## Amendment, 2026-08-21: nothing ambient, built before anything is granted

`--strict-mcp-config` is described above as riding alongside a generated `--mcp-config`, which
made it sound like part of the `Resource` work and therefore something that arrives when that
does. It is not. The flag is what makes a run's reach *bounded at all*, and until it was passed
the fleet had the opposite of this ADR's rule: every run inherited whatever MCP servers the host
user had configured for themselves, plus any `.mcp.json` the repo shipped.

Measured on `claude 2.1.238`, on an ordinary developer laptop rather than a contrived one: a run
spawned without the flag reported five MCP servers — a repo-supplied one and four of the owner's,
three of them connected, mail and calendar among them. So "what a run can reach depends on which
machine won the bid" was literally true, and a repo could grant itself a service by committing a
file. With the flag and no `--mcp-config` the run's MCP set is empty; with one, it is exactly what
was named.

It is therefore **unconditional and not configurable**, passed on every argv this adapter builds —
asserted for fresh, resumed and `Full`-permission requests alike, because the failure is silent
and a resumed run that lost the flag would be the more dangerous one, having already been trusted
once. Verified end to end through the daemon, on a machine with both kinds of ambient server
present: the spawned process carries the flag, and the agent itself reports it has no `mcp__`
tools.

The rest of `Role::Resource` — nominating a resource, granting it per run, and proxying a call to
the node that holds it — is unbuilt and unchanged by this. What changes is the order: the fence
came first, so that granting something later means something.

## Amendment, 2026-08-21: a resource on *this* node, granted per run

The local half of `Role::Resource`, built after the fence above. What a resource *is* on a
general-purpose machine is the same question a sink faced and gets the same answer: **an MCP
server the owner nominated** (`[[resources]]`), because there is no mail client and no credential
store in this tree, and the one thing the owner can verify is that a program exists. The service
is their declaration and the command is only how it is invoked — which is what lets a run ask to
reach email without naming a script on a particular machine.

**`offload run --use email`** is the grant, by service and never by id, for the audience's reason:
an id is a node's own name for one of its own things. It reaches the run in two places.

**Placement**, via a new `Constraint::CanUse { service }`. Until a resource can be reached across
the mesh, the only node that can honour a grant is the node holding it, so a granted run is placed
where the resource is or refused while the operator is still at the keyboard (ADR-0014) — never
accepted somewhere it would quietly reach nothing. The variant exists because `HasService` cannot
express it in either setting: `role: Some(Resource { access })` is matched exactly, so a run
wanting to *read* would decline a node offering read-write; and `role: None` is worse, because a
phone with an email **sink** satisfies "has email" and a run that wanted to read a mailbox would
be placed on a device that can only send to one. Access describes the grant and does not select
it — the owner knows what their integration does and the person submitting a run does not.

**The spawn**, where the grant becomes an MCP config and — the part the first end-to-end run
corrected — **permission to call it**. Projecting only the config gave the agent a tool it could
see and was then denied: `--use email` made a mailbox visible and unusable, and the run reported a
declined permission request. A grant that grants nothing is worse than no grant, because somebody
made a decision and it did not take effect. So one function returns both, and the pattern is
`mcp__<id>` — whole-server, which is the grant's own granularity rather than a shortcut, since
nobody submitting a run knows which tools a node's integration happens to expose. Narrowing is
`--allow`'s job, layered over this the way it layers over everything else.

Both halves resolve **on the holder**, not at submit, because the tool pattern is spelled with a
node's own name for its own resource. A migrated run is re-projected against the new holder's
resources, which is the same sentence as *nothing about a resource ever travels*.

One thing this taught the tool allowlist: `mcp__mail` is the agent's spelling for every tool that
server offers, so `ToolAllowlist::covers` now reads the `mcp__` prefix. Without it our matcher
disagreed with the agent about the one pattern this project generates for itself — the drift a
second implementation of somebody's pattern language exists to be worried about, arriving through
our own front door. Anchored on the separator, so one server's grant cannot reach another's tools.

**Verified against a real agent and a real stdio MCP server.** Granted, the run called
`mcp__mail__read_inbox` and reported who the message was from; ungranted, the same prompt found no
such tool; and `--use calendar`, which nothing in the fleet offers, was refused at submit naming
the constraint.

One correction to the shape above, found by auditing what this session had itself just added:
the generated configuration goes to the agent as a **file path**, not as JSON on the command
line. A resource's `env` is exactly where a credential for the service lives, and
`/proc/<pid>/cmdline` is readable by any local user on a default Linux — measured on this one.
The node writes it `0600` under its state directory and removes it when the leg ends; the
settings block beside it stays inline, because it carries nothing secret. "Credentials do not
migrate" was already the rule for the network; this is the same rule turned toward the machine.

## Amendment, 2026-08-21: the proxied call, and the constraint it retires

Built. The phone holds the mailbox and hosts nothing; the desktop runs the agent and forwards the
call. The agent is handed an MCP server whose program is `offloadd` itself — the ask hook's trick,
for its three reasons: nothing to discover, nothing to configure, and no way to be pointed at a
different version of the daemon than the one that spawned the agent. Each line of the agent's own
protocol goes to the holder and each answer comes back, opaque at every hop, because a proxy that
understood MCP would be a second implementation of somebody else's spec kept in step by hand.

**The prediction above held, with one exception.** Adding it changed where a call goes and nothing
about what a grant means — `--use email` is the same flag, the grant is still per run, still by
service, still projected as a config *and* permission to call it. What it did change is
`Constraint::CanUse`, and the ADR's own words are what retired it: a grant constrained placement
"until a resource can be reached across the mesh, [when] only its holder can honour one". Keeping
it afterwards would have defeated the case this ADR was written for, because the device holding
the mailbox is exactly the one that cannot host the run.

What replaces it is a check at **submission** rather than a constraint at placement, which is
ADR-0014's argument moved to where it belongs: a service *nothing in the fleet* offers is refused
while the operator is still at the keyboard, and one somebody offers is reached from wherever the
run lands. `Constraint::CanUse` stays in the vocabulary — "place this where the resource itself
is" is a coherent thing to want — and nothing constructs it automatically any more.

Two details worth keeping.

**A proxied server has no `env`, and that is the whole feature.** A local resource's environment
is where the token for the service it reaches lives; a remote one has none, because only the call
moves. So the agent-visible name is the *service* rather than the holder's own id for it, which
this node does not know and has no business learning — and as a side effect the tool prefix is the
same on every machine, so a migrated run's transcript stays readable.

**Who may use it is bounded by membership, not by the message.** The holder checks that the asking
run's record grants the service, and that check reads the asker's own gossip: a member could
publish a record granting itself the world. It is done anyway, because it catches what will
actually happen — a run reaching for something it was never granted, after a migration or a
misconfiguration — and it makes the refusal say so. What actually bounds this is that the fleet is
one person's devices, admitted by a certificate the fleet key signed, and the answer to a member
that lies is `offload revoke`. The alternative — signing run specs so a holder could verify a grant
it did not issue — is a second trust hierarchy for a fleet that has exactly one, and ADR-0012
rejected that shape for membership already.

Verified end to end on two daemons: a run submitted on the desktop with `--use email`, placed on
the desktop, called `mcp__email__read_inbox`, and the phone's log shows it opening the mailbox and
carrying the call. The same prompt without the grant found no `mcp__` tools at all, and
`--use calendar` — which nothing in the fleet offers — was refused at submission.
