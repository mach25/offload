# Offload

Orchestration for **coding agents** across a personal device fleet.

You submit an agent run — a repo, a prompt, an agent (Claude Code, etc.) — and the fleet
negotiates over who takes it. Devices (phones, laptops, desktops, VMs) join and leave freely.
When a device leaves, its in-flight runs **migrate**: the session transcript and the workspace
move to another capable device and the agent resumes mid-conversation.

Close the laptop, the agent keeps working on the desktop.

Two binaries:

- `offloadd` — the node daemon. One per device. Joins the mesh, hosts agent runs. *(phase 1)*
- `offload` — the operator CLI.

## Read first

- `docs/ARCHITECTURE.md` — the system model. Read before touching `offload-cluster`,
  `offload-cluster::place`, or `offload-agent`.
- `docs/HANDOFF.md` — the live head: what is true now, and what to pick up. Short on purpose, and
  read before starting anything. The owner's own fleet is in `local/SETUP.md` (not committed).
- `docs/ROADMAP.md` — what is **not** built, and the open questions with the phase each blocks.
  Authoritative on both, and short. What a completed phase shipped is in `docs/phases.md`.
- `docs/adr/` — settled decisions, and the reasoning that is easy to lose. Don't relitigate
  one without adding a superseding ADR.
- `docs/pitfalls/` — what previous sessions got wrong, by subject. The index is at the bottom
  of this file; **open the file covering what you are about to touch, before you touch it.**
- `docs/GLOSSARY.md` and `docs/COMMANDS.md` — the long form of the two sections below.
- `docs/ANDROID.md` — for a person, not a session: building the app, installing it, joining a
  fleet. Keep it true when the app's screens or the build change.
- `docs/use-cases/` — workloads somebody actually wants, explored against the code. Not decisions:
  each one says what already fits, what does not, and which of the gaps is worth an ADR. Read one
  before designing for the workload it covers.
- `docs/sessions.md` (by session) and `docs/phases.md` (by phase) — why the tree is the way it is.
  Read when you need the reasoning behind something, not at the start of a session. `docs/DEMO.md`
  runs the daemons.

## How to work here

1. **Read `docs/HANDOFF.md`'s live head** — state of the tree, then the pick-up list. It is
   written for whoever starts next, which is you.
2. **Check `docs/ROADMAP.md` for the phase** a crate is in before assuming a thing exists.
3. **Read the pitfall file for what you are about to touch** — the table at the bottom of this
   file maps path to file. Every entry in it is a mistake somebody already made here.
4. **Measure rather than argue.** Nearly everything in `docs/pitfalls/` was found by running two
   daemons and reading the output, and *none* of it by reasoning about the code. A doc comment in
   the past tense is not evidence; a passing test on a fixture is not evidence. Walk it.
5. **Say where things stand when a task is done.** End every completed task with a short status
   the person reading it can orient from without opening a file: what the task changed, the
   **phase** and whether it moved, the **ADR** it added, amended or closed a residual of (or
   *none*, said explicitly), the test/clippy state, and the one thing to pick up next. Keep it to
   a few lines — it is a position on the map, not a second changelog. `docs/ROADMAP.md` and
   `docs/HANDOFF.md` are still the authorities; this is the pointer into them, and if it takes
   long to write, the handoff needed replacing anyway.
6. **Write down what went wrong**, not just the fix: a pitfall entry for a rule the next session
   would otherwise break, an ADR for a decision, a paragraph at the top of `docs/sessions.md`, and
   a replaced — not appended — pick-up list in `docs/HANDOFF.md`. `CONTRIBUTING.md` has the full
   table and the commit-message conventions.
7. **Commit onto `main`, which stays local.** No session branches. `origin` (github.com/mach25/offload,
   public) is published from a separate branch, `public`, one squashed commit per publish, so the
   private history never leaves this machine. **Never push `main`.** To publish: scrub first (no
   real addresses, paths, serials, ids or device models; see `CONTRIBUTING.md`), then
   `git commit-tree main^{tree} -p public -m …`, move `public` to it, and `git push origin
   public:main` only when the owner asks.
8. **The owner's setup is local, never committed.** Their devices, addresses, serials, account and
   fleet ids, device models and client project names live in `local/` (gitignored):
   `local/SETUP.md` for the fleet as it stands and how to reach each device, `local/forbidden.txt`
   for the real values. Tracked files use placeholders (`192.0.2.x`, `phone-app`, `tablet`,
   `PHONESERIAL`, `/home/owner`, "the phone"). A local pre-commit hook refuses a commit that adds
   a listed value; it is a backstop, not the rule. Read `local/SETUP.md` before operating the
   fleet. A checkout without `local/` belongs to somebody else: ask, never guess.

## Workspace layout

Every crate listed exists.

```
crates/
  offload-core/       domain types: capabilities, constraints, run state machine, bidding,
                      run progress and how it merges
  offload-probe/      local capability detection (shells out, reads /sys)
  offload-cli/        the `offload` CLI
  offload-agent/      agent adapters: spawn, stream events, turn boundaries, resume
  offload-workspace/  per-run git worktrees over a shared per-repo bare mirror
  offload-store/      SQLite state: run registry, event log, blobs, absence history, audit log
  offload-node/       the `offloadd` daemon: config, control socket, run supervision
  offload-proto/      wire messages: framing, handshake, version negotiation
  offload-transport/  Transport trait, in-memory and QUIC implementations
  offload-cluster/    membership, gossip, failure detection, discovery — and `place`, the bid
                      round (`tests/storm.rs` drives a generated fleet of *real* clusters over
                      the in-memory transport — the churn properties one layer down)
  offload-ios/        the daemon as a static library with a two-function C ABI, for the iOS
                      host app in `ios/` (ADR-0070)
  offload-mobile/     the product apps' client: the control socket's types as UniFFI screen
                      models, for the Compose app in `android-app/` (ADR-0071)
  offload-power/      keeps the machine from sleeping while the node may take work (ADR-0077);
                      the one crate besides `offload-ios` allowed `unsafe`, in `src/macos.rs` only
```

Two Android projects, on purpose: `android/` is the minimal host harness walks drive over adb
(ADR-0066), and `android-app/` is the product app (ADR-0071). Do not grow the first into the second.

There is deliberately **no `offload-sched`**, though the roadmap once planned one: the bid round
needs the view, the connections and the serve loop, all of which `offload-cluster` already has.
Create one when migration *policy* grows past what `offload-core` already decides, and not before.

Dependency direction is strictly downward: `core` ← `proto` ← everything else. `offload-core`
has no I/O, no tokio, no networking, **no clock** — pure domain logic, because that is what
the simulation tests exercise. Probing lives in `offload-probe` for exactly this reason.

## Vocabulary

Use these words precisely; they map to types. One line each here; **`docs/GLOSSARY.md`** carries
the reasoning behind every one, and the reasoning is usually the part that matters.

- **Run** — one agent execution; the unit of scheduling and migration. Not "task" — that word is
  taken (below). Its turns, cost and denials are `RunProgress`, beside the record; **position**
  and **spend** merge by different rules (ADR-0005).
- **Agent** — the thing being orchestrated (`AgentKind::ClaudeCode`). Spawned, streamed, resumed —
  never reimplemented.
- **Workspace** — the filesystem the run acts on: a git repo at a ref, in a dedicated worktree.
  Migrates with the run.
- **Session** — the agent's own conversation state, addressed by session id, captured as a
  transcript blob. What makes resume-mid-conversation possible.
- **Capabilities** — what a device *is and has*. Ability, not permission. Instances with roles —
  `Sink`, `Trigger`, `Resource` (ADR-0011).
- **Work policy** — what the device's *owner* permits: charging, battery floor, concurrency cap,
  budget, metered data. Separate from capability on purpose.
- **Attendance** — whether anybody is watching: *observed*, never declared (ADR-0013). Decides
  autonomy in failure. Never gossiped.
- **Origin** — `Operator` or `Rule` (ADR-0024). Immutable, so it gossips cleanly. Decides only what
  may be **thrown away**; must never reach bidding, placement, capacity or delivery.
- **Demand** — what hosting is expected to cost: `Light | Normal | Heavy`. A hint; hardware a run
  cannot do without is a `Constraint`.
- **Constraint** — what a run *needs*: a boolean tree over capabilities. Keep `explain` in step
  with `matches`.
- **Bid** — a node's self-assessed offer. Nodes bid, they are not assigned to. Carries an
  `Availability` as well as a score.
- **Accepted** — a node holds the run under a lease, started or not. Accepted or refused while the
  operator is still there (ADR-0014); a full node *commits* rather than declines.
- **Arbiter** — who grants a run to a bidder: its home node until that node is *gone*, else the
  lowest-id available node. Per run, not per fleet.
- **Stability** — how much to trust a node to stick around. Affects *scoring*, never eligibility.
- **Fleet** — the enrolled devices, defined by an ed25519 signing key rather than a registry
  (ADR-0012). Membership is a certificate, so it need not travel; **revocation** must.
- **Approver** — a device delegated the right to issue certificates, so the passphrase stays in a
  drawer. An issuer, never a root; may not mint `host-runs`.
- **Succession** — the new fleet's identity signed by the key it replaces. What lets `offload
  rekey` *move* a device that already belongs.
- **Lease** — a node's time-bounded right to hold a run. Renewed by heartbeat.
- **Epoch** — monotonic fencing token, bumped on every assignment and release. Monotonic *per
  arbiter*, which is weaker than a total order.
- **Orphaned** — holder out of contact, **no decision made yet**. Grants no authority, returns no
  lease; a returning holder reclaims at the same epoch.
- **Turn boundary** — between agent turns. The only safe place to checkpoint (ADR-0004).
- **Allowlist** — per-tool grants layered from node config, the repo's `.offload.toml`, and
  `--allow`. Repo-supplied grants are capped.
- **Drain** — graceful departure: stop accepting, checkpoint at the next boundary, hand off.
- **Sink** — a route from a run to a human, advertised as a capability. The command stays on the
  node; a route may be on *another* node, which is the point.
- **Trigger** — a nominated program that notices something happening. One line of stdout is one
  event, and there is deliberately no interval (ADR-0020).
- **Task** — a run whose work is a nominated program, not an agent: no prompt, no workspace, no
  model, no tokens (ADR-0019 §2). The middle of three tiers of graduated cost, and **all of it
  runs** — submitted (`offload run --task`), fired by a trigger or a schedule, placed by the
  ordinary bid round, gated on the owner's `[policy.light]`. A failed one is **restarted**, never
  resumed: there is no conversation to continue, so `offload resume` refuses it and the recovery
  tick runs the program again from the spec (ADR-0058).
- **Rule** — *when this fires here, submit this run* (`offload when`). Fired by a trigger's line
  or by a **notice** this node projected (ADR-0057) — an escalation is a *delivery*, so it gets
  the outbox's cursor, dedup and retries. Node-local, never gossiped, one occurrence at a time,
  prunes what it leaves behind. A notice about machine-started work fires nothing.
- **Schedule** — *run this every so often* (`offload every`). A rule's opposite in the one way
  that matters: **gossiped**, so it outlives the device that wrote it, which is why it needs an
  owner, a successor and a tombstone (ADR-0056). Its occurrence's `RunId` is derived from
  `(schedule, tick)`, so two nodes firing one tick converge on one record. No catch-up.
- **Resource** — something on a device a run may be granted (ADR-0011): granted per run, by
  service, projected as an MCP config *and* permission to call it. Does not constrain placement.
- **Notification** — the small typed subset worth interrupting a person for, projected from the
  event log and fanned out through an outbox, deduplicated on `(subject, seq, sink)`.
- **Question** — a run stopped mid-tool-call waiting for permission (ADR-0017). Opt-in, bounded,
  answerable from any device, addressed to the agent's own `tool_use_id`.
- **Audience** — which routes a run's news is for, named by *service* and never by route id.
  Decides who is interrupted, never what is recorded. `Notices` is the other axis (ADR-0026).

## Working agreements

- **Async only where I/O happens.** `offload-core` is synchronous and deterministic. Every
  decision — placement, bidding, reassignment — is a pure function of (view, run, now).
- **Time is injected**, and `offload-core` is checked for it. Never call `SystemTime::now()` /
  `Instant::now()` in core logic. Take a `Millis`. Anything reading the wall clock directly is
  untestable here — which is the quiet part: the test that would have caught it is the one that
  stops being possible. `offload-core/tests/no_clock.rs` reads the crate's own source, because
  the rule is about what the code may *call* and there is no type that says so. `offload-store`
  is the one exception — rows carry timestamps and `now` has to come from somewhere.
- **`offload-store`'s API is synchronous.** SQLite's is; wrapping it in `async` would hide
  the cost, not remove it. Callers on an async task use `spawn_blocking`.
- **Migrations are append-only**, and now pinned. Never edit one that has shipped: nodes upgrade
  at different times, so an edited migration means two nodes disagreeing about their schema while
  `user_version` says they agree — silent, with no error to notice. A known-answer test in each
  list (`offload-store::schema`, `offload-node::broker`) digests every shipped migration and
  fails on an edit; adding one adds a digest, and a new migration with no digest fails too.
- **Never reimplement an agent.** `offload-agent` shells out and speaks the agent's own
  protocol (Claude Code: streaming JSON, `--resume <session-id>`). If we find ourselves
  parsing model output or managing conversation state, we have taken a wrong turn.
- **Decisions carry reasons.** Return `Hold { until, reason }` and `NoBid::Refused(..)`, not
  bools and `None`. Nearly every user-facing question here is "why didn't that happen", and a
  decision that discarded its reasoning cannot answer it.
- **Every new gossiped field needs an owner.** Write down who is authoritative and what
  arbitrates it (see the table in ADR-0005). A field whose owner nobody decided is a merge
  bug waiting to happen. And check the owner still exists at the moment the fact is final:
  `RunProgress` first deferred to the run's *holder*, which is nobody once the run has
  finished — so the last numbers a run ever produces were the ones no node was entitled to
  state. What replaced it was forward-only, which had the opposite failure: see the entry about the
  leg that lost, in `docs/pitfalls/gossip-and-merge.md`. Sometimes one field is two facts with two owners.
- **No unwrap/expect outside tests and `main`.** `thiserror` in libraries, `anyhow` in
  binaries. Enforced: `unwrap_used` and `expect_used` are workspace lints and clippy runs with
  `-D warnings`. Note where that does *not* reach — a `#[cfg(test)]` module inside a lib is
  exempt, an integration test under `tests/` is not.
- **Tracing, not println.** Structured fields (`run_id = %id`). Agent stdout is data, not
  logging — it belongs in the run's event stream.

## Commands

The four that run on every change:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

**`docs/COMMANDS.md`** has the rest: the harder property runs, the local no-daemon questions
(`probe`, `policy`, `match`), the whole membership set, and the daemon-and-a-real-run set. Read it
before running a walk, and before assuming a command exists.

## Things that are easy to get wrong

Every entry in `docs/pitfalls/` was paid for. A rule there is either enforced by something or it
is a hope, and the two read identically — where a rule is mechanically checkable the entry says
what checks it, and where it is not, that is worth knowing too. `--strict-mcp-config` sat in that
list for two phases as a warning about a hole that was open the entire time.

**Read the file before you edit the code, not after the walk goes wrong.** An index line is a
handle for recall, not the lesson. Each file is the *rules* — a few lines each — and most are
1–4k tokens. **Four are not**: `reports-and-cli` (below), `lifecycle-and-recovery` ~8.3k,
`capacity-policy-and-probing` ~8.0k and `checkpoints-blobs-and-workspaces` ~6.5k (re-measured
session ninety-two, bytes/4). `reports-and-cli` had reached ~15k, up from ~9k nine
sessions earlier, having grown in **every** session that read it for six measurements running —
and was split session ninety into three subjects by content rather than by size:
`reports-and-cli` itself (~9.7k at the split, ~10.1k two sessions later — the general "a report
has to come from where the decision reads" cases — `ps`/`explain`/`logs`/`cancel`/`rm`/`resume` and run-state wording),
`refusals-and-instructions` (~3.8k — placeholders, ids vs names, first-person sentences, config
and parser errors, help text) and `logs-and-listings` (~2.1k — `offload audit`, `nodes
--history`, timestamps, truncation, column widths). `reports-and-cli` is still the largest pitfall
file and nowhere near the small ones; re-measure rather than assume the split settled
it. Read the whole of a small one; on the large files, read the whole file the first time you
touch its subject in a session and skim by opening line after that.
`docs/pitfalls/detail/` holds **most** of the same entries in full — the mechanism, the
measurement, and how each was found. Open a detail entry when the rule alone is not enough to act
on, not by default, and **find it by searching its opening text rather than by counting to the
same position**: the two files are appended to together but the rule file has often gained an
entry the detail file did not, so the Nth detail entry stops being the Nth rule early (in
`capacity-policy-and-probing`, by the third). Re-measured session ninety-three: **502 rules, 451 detail entries** — counting a detail
entry as either a `- **` bullet *or* a `## ` heading, since several files use both. **Paired by
subject rather than counted, every file is complete** except for eight rules with no source beyond
a commit diff, named at the head of each file's backfill. One detail entry often covers several
rules, which is why the count stays below the rule count. The gap is an upper bound on what is missing rather than a
count of it, because a continuation rule (`…and …`) often shares one detail entry with the rule
above it, so a file at parity is not evidence of a file at parity either way.

*(That sentence said "the largest is well under 2k tokens" for several phases, while the largest
was over three times it — the same drift the roadmap's own table had. A number in a doc is a
measurement with a date on it; re-take it rather than inheriting it.)*

| Before you touch | Read |
| --- | --- |
| `offload-node/src/supervisor.rs` — anything that starts, stops, fails or describes a run | `fencing-and-epochs`, `lifecycle-and-recovery`, `capacity-policy-and-probing` |
| `offload-core/src/run.rs` — epochs, leases, grants, `holds`/`describes`, terminal states | `fencing-and-epochs` |
| `offload-core/src/{view,progress}.rs`, `offload-cluster/src/{lib,members,detector}.rs` | `gossip-and-merge` |
| `offload-transport/src/{quic,sends}.rs` — the socket, and what it does with an error | `reports-and-cli`, `gossip-and-merge` |
| `offload-node/src/{mesh,leftovers,statedir}.rs`, `offload-core/src/recovery.rs` | `lifecycle-and-recovery` |
| `offload-core/src/fleet{,.rs}`, `offload-node/src/{fleet,identity}.rs`, `offload-cli/src/fleet.rs` | `membership-and-credentials` |
| `offload-core/src/{policy,capacity,bid,capability,constraint}.rs`, `offload-probe/` | `capacity-policy-and-probing` |
| `offload-agent/`, `offload-core/src/{allowlist,ask}.rs`, `offload-node/src/{resource,asks}.rs` | `agent-adapter` |
| `offload-core/src/{notify,fleet_event}.rs`, `offload-node/src/deliver.rs`, `offload-store/src/deliveries.rs` | `delivery-and-notifications` |
| `offload-node/src/{trigger,schedule}.rs`, `offload-store/src/{rules,schedules}.rs`, `offload-core/src/schedule.rs`, `offload-cli/src/when.rs` | `rules-and-retention` |
| `offload-workspace/`, `offload-store/src/blobs.rs`, `offload-cluster/src/blobs.rs` | `checkpoints-blobs-and-workspaces` |
| `offload-store/src/{schema,runs}.rs`, `offload-proto/`, `offload-core/src/id.rs` | `storage-and-encoding` |
| `offload-cli/src/commands.rs`, `offload-node/src/{explain,api}.rs`, `offload-core/src/audit.rs`, **any operator-facing string** | `reports-and-cli`, `refusals-and-instructions`, `logs-and-listings` |
| `offload-core/src/deadline.rs`, `offload-cluster/src/place.rs` — urgency, attendance, `Pending`, ordering | `scheduling-and-attendance` |
| `offload-power/` — keeping a host awake | `testing-and-sweeps` (walk it by the machine, not the assertion) |
| Any test, simulation, property or sweep — and before trusting one | `testing-and-sweeps` |

A new entry goes in the file whose subject it belongs to; a new file only for a subject none of
them covers, and then a row here.

Six that are worth carrying without opening a file:

- **Double execution is the failure mode that matters.** Two agents on the same repo, both
  committing, is worse than no agent. Every side-effecting path checks the epoch. When in doubt,
  prefer a run stalled over a run duplicated.
- **A fence after the effect is not a fence**, and one whose failure is discarded is not one
  either. When a guard and the thing it guards are separated by an `await`, the guard is on the
  wrong side of it.
- **A node must not believe a peer about itself** — its liveness, its own facts, or a run it made
  and deleted here.
- **A report has to come from where the decision reads.** A command that tells somebody what the
  system will do, computed from a second copy of the rule, is confidently wrong and silent.
- **Unknown is not none, and unknown is not good news.** Collectors, attendance, adoption and
  auto-resume each take the safe answer loudly rather than the convenient one.
- **Don't over-claim** — in the probe, in a doc comment, in a status line. A node that claims what
  it lacks wins bids and then fails every run it takes.
