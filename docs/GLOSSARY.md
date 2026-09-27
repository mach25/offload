# Glossary

Words that carry specific meaning in this project; they map to types. `CLAUDE.md` carries the
one-line form — this is the reasoning behind each.

- **Run** — one agent execution. The unit of scheduling and migration. Not "task" — never as a
  synonym for this, and since ADR-0019 that word is *taken*: see **Task** below, which is a run
  of a different kind rather than another word for this one. Its turns, cost and denials are
  `RunProgress`, kept beside the record rather than in it: a resumed or migrated run continues
  the count instead of starting one. Two kinds of number, and they merge by different rules
  (ADR-0005's second amendment): **position** — the turn it is on, what its worktree holds —
  belongs to the run's current *leg* and is settled the way the record is, by epoch and then by
  lowest author id; **spend** — money, denials, questions put to a person — belongs to nobody
  and only grows, because a leg that lost still spent it.
- **Agent** — the thing being orchestrated (`AgentKind::ClaudeCode`). Offload spawns it,
  reads its event stream, and resumes it. It does not reimplement it.
- **Workspace** — the filesystem the run acts on: a git repo at a ref, in a dedicated
  worktree. Migrates with the run.
- **Session** — the agent's own conversation state, addressed by its session id and captured
  as a transcript blob. What makes resume-mid-conversation possible.
- **Capabilities** — what a device *is and has*. Ability, not permission. Includes how it
  can *report* as well as how it can work: a delivery route is a capability (ADR-0010), so
  a phone that can push a notification but not host an agent is a useful fleet member.
  Instances, not flags (ADR-0011) — each carries an identity, a verified auth flag, and one
  or more **roles**: `Sink` (reach a human), `Trigger` (something arrives, work starts),
  `Resource` (a run acts on it). Same service, different roles, different grants.
- **Work policy** — what the device's *owner* permits: accept-when-charging, battery floor,
  concurrency cap, share budget, metered-data refusal. Separate from capability on purpose — a
  phone that says "not right now" is exercising policy, not lacking capability, and the two
  produce different messages.
- **Attendance** — whether anybody is watching a run: *observed*, never declared (ADR-0013). A
  run is attended while some client streams it, and what that decides is **autonomy in failure**
  — unattended picks itself back up, attended stays broken in front of the person who is sitting
  there. Observed at the moment the run fails, because a failure ends the stream a follower was
  reading; held in memory by the node that failed it, and gossiped nowhere.
- **Origin** — what kind of thing started a run: `Operator` or `Rule` (ADR-0024). Set once when
  the run is created, changed by nothing, and therefore the cleanest kind of gossiped field —
  identical in every copy, so there is no owner to name and no merge rule to get wrong. **Not
  attendance**, which is *observed* and deliberately never gossiped: this is known at submission
  and immutable. It decides exactly one thing — what may be **thrown away** — and must never
  reach bidding, placement, capacity or the delivery plane, because a triggered run is an
  ordinary run and that is ADR-0020's whole claim.
- **Demand** — what hosting a run is expected to *cost* a device: `Light | Normal | Heavy`
  (ADR-0013). Coarse because the workload is a model with a shell, so a finer number is a
  fiction with decimal places; a hint rather than a requirement, because hardware a run cannot
  do without is a `Constraint`. A device has a budget of shares beside its run count — the
  count stays a hard ceiling — and admission takes the harsher of what the run *declared* and
  what the machine's load average *shows*.
- **Constraint** — what a run *needs*. A boolean tree over capabilities. Keep
  `Constraint::explain` in step with `matches`; "why is my run still pending" is the top
  debugging question and a drifted `explain` is worse than none. `satisfied` cannot drift — it
  calls `matches` — so the drift is in the *words*: `observed` has to say what the node actually
  has, and its arms are written out so a new variant fails to compile rather than silently
  explaining only what was asked for.
- **Bid** — a node's self-assessed offer to host a run. Nodes bid, they are not assigned to.
  It carries an `Availability` as well as a score: a node at its cap bids *and* says it cannot
  start yet, because being full is a queue that empties rather than a refusal.
- **Accepted** — a node holds the run under a lease, whether or not the agent has started yet.
  A submission is accepted by somebody or refused to the operator while they are still at the
  keyboard (ADR-0014); the third outcome exists only when asked for by name (`--queue`), and
  then the run is offered again until somebody takes it, because "filed away, good luck" is the
  babysitting this project exists to remove. A full node therefore *commits* — it stays
  `Assigned`, heartbeating, and starts when a slot frees — rather than declining and leaving
  nobody holding the run.
- **Arbiter** — who grants a run to a bidder. Its home node until that node is gone, else the
  lowest-id available node. Per run, not per fleet.
- **Stability** — how much to trust a node to stick around (`Ephemeral` phone → `Stable` VM).
  Affects *scoring*, never eligibility.
- **Fleet** — the set of enrolled devices, defined by an ed25519 signing key rather than by
  a registry (ADR-0012). Membership is a certificate that key signed, so a peer verifies a
  stranger offline, on first contact — which is why **membership is the one thing here that does
  not have to travel**. Revocation is the exception, because it is the *absence* of a signature
  and no certificate can carry it: it gossips, it is fleet-owned, and — unlike liveness — the
  subject can never refute it. Certificates last thirty days and are **renewed on contact** by an
  approver, which is what makes an unheard revocation eventually bite. Enrolling needs no
  passphrase when an approver is holding one end (`offload invite`), and no network in either
  case: what crosses is a certificate, not a secret.
- **Approver** — a device the fleet key delegated the right to issue certificates to, so the
  passphrase stays in a drawer (ADR-0012). It is an *issuer*, never a root: losing every approver
  loses a safeguard and never the ability to operate, because `offload join --passphrase` and
  `offload rekey` always work. It signs invitations and renewals under its delegation; `host-runs`
  it may not issue, because that one grant stays behind the passphrase — **enforced by the
  verifier**, which is what makes it true of a compromised approver and not only of an honest
  one. It may *renew* a certificate carrying one, because a renewal carries the fleet-signed
  certificate it restates and may not exceed it.
- **Succession** — the new fleet's identity signed by the key it replaces, carried by `offload
  rekey`'s invitations. What lets a device that already belongs be *moved*: without it, an
  invitation to an unfamiliar fleet is indistinguishable from somebody trying to take the machine,
  and the only safe answer is no.
- **Lease** — a node's time-bounded right to hold a run. Renewed by heartbeat.
- **Epoch** — monotonic fencing token, bumped on every assignment and on release. A node
  acting under a stale epoch must have its side effects rejected. This is what makes it safe
  to reassign a run whose holder we merely *believe* is gone.
- **Orphaned** — holder out of contact, and **no decision made yet**. Grants no authority.
  A returning holder can reclaim it at the same epoch, costing nothing.
- **Turn boundary** — between agent turns. The only safe place to checkpoint (ADR-0004).
- **Allowlist** — per-tool grants layered from node config, the repo's `.offload.toml`, and
  `--allow`. Lets a run execute its own tests without the general execution `Full` grants.
  Repo-supplied grants are capped: a repo may say "run my tests", not "give me `sh`".
- **Drain** — graceful departure: stop accepting, checkpoint at the next boundary, hand off.
- **Sink** — a route from a run to a human: push, email, chat, a webhook, an attached terminal.
  Advertised as a capability, never carrying its credentials to another node. Today it is a
  **command the node's owner nominated** (`[[sinks]]` in the node config) — the only route a
  general-purpose machine can honestly claim, since the one thing it can verify is that the
  program exists. The *service* is the owner's declaration and the *command* is only how it is
  invoked, which is what makes "reach me by push" something a run can ask for. The command stays
  on the node: a peer learns this device *has* a push route and never how it works, so a route
  with no description of its own gossips none rather than falling back to the program's path.
  **A route may be on another node** — that is the point: the run finishes on the desktop and the phone, which
  hosts nothing, is what reaches you. The sender keeps the outbox because it is the only node that
  knows what it has said; the peer is told *what* to say and never *how*.
- **Trigger** — a program the node's owner nominated that notices something happening
  (ADR-0011's third role, settled by ADR-0020). One line of its stdout is one event, and that is
  the whole protocol: there is deliberately **no interval**, because a daemon that polled on a
  schedule would be a scheduler inside an orchestrator, and something that fires on a clock is a
  loop the owner wrote. Nominated in node config (`[[triggers]]`), advertised by *service*, and —
  like a sink and a resource — the command never leaves the node.
- **Task** — a run whose work is a program the owner nominated rather than an agent: no prompt,
  no workspace, no model, and no tokens (ADR-0019 §2's `Work::Task { service, args }`).
  **Built, and all of it runs** — a node with no agent installed wins bids for one and executes
  it, a person submits one, and a **rule** or a **schedule** fires one. It is the middle of three
  tiers of graduated cost
  (`docs/ARCHITECTURE.md`): a **trigger**
  notices for free, a task evaluates for the price of a process, and an **agent run** is what
  costs money. Nominated in node config (`[[tasks]]`) and asked for by *service*
  (`--task watch-api`), never by command line, exactly as a sink, a trigger and a resource are.
  It has no turn boundary, so it has no checkpoint and cannot be `Resumable`. Which is why a
  failed one is **restarted** rather than resumed (ADR-0058): the recovery tick runs the program
  again from the spec, at a fresh epoch, and `offload resume` — the door a person types at —
  refuses it, because resuming means continuing a conversation and there is none.
- **Schedule** — a standing instruction on a clock: *run this every so often* (`offload every`),
  and the counterpart to a **rule** in the one way that decides everything about it. A rule is
  node-local because a trigger is a program one machine's owner nominated; a clock is everywhere,
  so a schedule is **gossiped** — which is the whole difference between it and `cron`, and which
  is what forces it to have an owner (the node it was created on), a successor rule
  (`ClusterView::steward_of`, arbitration's own) and a **tombstone** for removal, since a gossiped
  set that simply stopped mentioning something would re-learn it from the next peer for ever.
  Its occurrences' ids are **derived** from `(schedule, tick)` so that two nodes firing one tick
  converge on one record — a duplicate record merges and a duplicate agent does not. An interval
  with an optional UTC offset, deliberately not cron: a timezone is a database and a disagreement
  (ADR-0056). No catch-up — a phone asleep for six hours wakes and fires once.
- **Rule** — a standing instruction: *when this service's trigger fires here, submit this run*
  (`offload when`). It holds a whole submission, and it lives on the node whose trigger fires it
  and is **never gossiped** — a trigger belongs to one machine's owner, so it fires on one
  machine, which is why this needs none of the machinery a gossiped schedule would. It fires
  **one occurrence at a time**: an event arriving while the last run is still going is dropped
  and counted, never queued, because a watcher's value is the current state. Its deadline is a
  *duration* rather than an instant; it reclaims its last occurrence's checkout when there is
  nothing uncommitted in it; and it **prunes the records** of every occurrence the delivery plane
  has finished with (ADR-0021) — completed rather than failed, nothing pending in the outbox,
  every live route's scan past its last event, and quiet for as long as a finished run is
  gossiped. `offload rules` prints what it is keeping, because a number that only grows is the
  one symptom every residual of that has.
- **Resource** — something on a device a run may be granted the use of (ADR-0011): the one role
  that hands a run something, so the only one granted *per run* rather than implied by placement.
  A node holding a mailbox does not mean every run that lands there may read mail. Nominated in
  node config (`[[resources]]`) as an MCP server the owner declared, because that is the sink
  rule applied to the other direction — the service is their declaration, the command is only how
  it is invoked, and none of it is ever gossiped. Granted by **service** (`offload run --use
  email`), and projected at the spawn as an MCP config *and* the permission to call it. It does
  **not** constrain placement: a resource on another node is reached by proxying the call to it,
  so the phone can hold the mailbox and host nothing. What is answered at the keyboard instead is
  whether the *fleet* has one at all.
- **Notification** — the small typed subset of events worth interrupting a person for:
  finished, failed, will-miss-its-deadline, a question, and — the two that are not about a run at
  all — a device joining the fleet and the passphrase being used. Not the event stream; projected
  from it (`offload_core::notify`) and fanned out through an outbox, at-least-once, deduplicated
  on `(subject, seq, sink)`. There are **two logs**, a run's and the fleet's, and they number
  independently: `Topic` is half of every key here for that reason. Never emitted as a side effect from inside an adapter, and never something
  a turn boundary waits on.
- **Question** — a run stopped mid-tool-call waiting for a person to permit something
  (ADR-0017): `offload run --ask`, out through the delivery plane, `offload approve|deny` back
  **from any device**, addressed to the agent's own `tool_use_id` so one answer permits one call
  and a stale one matches nothing. Opt-in, because the wait is mid-turn and therefore
  uncheckpointable, and bounded for the same reason. The agent's half is a `PreToolUse` hook whose
  program is `offloadd` itself; an answer typed elsewhere is forwarded to the holder, because the
  blocked process is on exactly one machine. `offload asks` canvasses rather than reading gossip:
  a question stops existing the moment somebody answers it.
- **Audience** — which routes a run's news is for (`offload run --notify push`), named by
  *service* and never by route id, because an id is a node's name for one of its own sinks and
  naming one would pin a run's news to a device. On the spec, so it travels with the run;
  `Everyone` by default, which is right for a fleet of one person's devices; `Nobody` for the run
  somebody is sitting and watching. It decides who is *interrupted*, never what is recorded.

