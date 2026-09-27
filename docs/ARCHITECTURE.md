# Architecture

## The problem

You want to run coding agents — Claude Code and friends — on more than one machine, without
thinking about which machine. The fleet is heterogeneous and unreliable by nature:

| Device  | Cores | Power   | Network       | Uptime            | Agent-relevant                  |
| ------- | ----- | ------- | ------------- | ----------------- | ------------------------------- |
| Phone   | 8     | battery | mobile, NAT'd | minutes           | thin toolchain, opts in on power |
| Laptop  | 8–16  | either  | roaming Wi-Fi | hours, lid closes | full dev env, authed            |
| Desktop | 16–32 | AC      | wired LAN     | days              | full dev env, GPUs, big builds  |
| VM      | 2–16  | AC      | public/tunnel | weeks             | headless, authed, always on     |

Every one of these is a **worker**, phones included. A charging phone is a perfectly good host
for a review, a triage pass, a doc rewrite, or a planning run — work that is bound by model
latency rather than by a build toolchain. What a phone needs is not exclusion but the ability
to *decline*, which is what owner policy provides.

An agent run is a bad fit for ordinary job schedulers in three specific ways:

1. **It is long and stateful.** Tens of minutes to hours, with conversation state that is
   expensive to lose and cannot be reconstructed from inputs.
2. **Its side effects are real.** It writes files, commits, pushes, opens PRs, calls APIs.
   Running one twice is actively harmful.
3. **Its requirements are about environment, not just resources.** "8 cores" is not the
   interesting constraint. "Has `claude` installed, authenticated, with `gh` logged in, and
   node 22 available" is.

Offload's bet is that this makes agent runs *unusually good* migration candidates despite the
above: an agent's entire meaningful state is a transcript plus a working tree, both of which
are small and serialisable. That is not true of a running process in general.

That is the hardest thing the fleet schedules, which is why it is described first. It is not the
only thing, and the section below is the wider frame the rest of this document sits inside.

## Work of graduated cost

The section above describes the expensive tier, and for most of this project's history it was the
only one. It is not the thesis. **Offload schedules work of graduated cost, and an agent run is the
most expensive kind it can schedule.**

| Tier | What it is | What it costs | Status |
| --- | --- | --- | --- |
| **Trigger** | a long-lived program the owner nominated; one line of stdout is one event | nothing until it prints | built (ADR-0020) |
| **Task** | a program the owner nominated, addressed by service: no prompt, no workspace, no model | a process | **accepted, unbuilt** (ADR-0019) |
| **Agent run** | everything the section above describes | tokens, a worktree, a transcript, migration | built |

The tiers compose into an escalation, and the escalation is the point: **a trigger notices, a task
evaluates cheaply, and an agent is called only when the cheap tier cannot decide.** A fleet that
can only run agents costs money while it sleeps. A fleet with a free bottom tier does not, and the
difference is not an optimisation — it decides which workloads are worth putting on it at all.

Stated as a principle a future feature can be tested against: **keep the model out of the loop
until it is the only thing that can help.**

`docs/use-cases/operations-fleet.md` is the same conclusion reached from a real workload rather
than from first principles — forty shops to keep healthy, where the answer to *"needs an agent at
all?"* is **"mostly no — it is a comparison"**, and the shape is *"arithmetic that occasionally
escalates"*. That it was derived twice, from opposite ends, is the reason it is here rather than in
a roadmap item.

### The kind of work is declared, never inferred

A submission says which kind it is. Offload does not guess, and nothing in the system infers the
kind from the presence or absence of a field — no "there is no repository, so it must be a task".

This is ADR-0019 §2's tagged `Work` enum rather than optional fields beside the agent ones, and the
ADR states the reason in the project's own accumulated experience: *"One field that is two facts is
the mistake this project has now fixed three times"* — `LogKind::Failed` borrowed for a failed
capture, one signal carrying both "stop the agent" and "the run is over", and a run's position
sharing a merge rule with its spend. An enum makes the impossible combinations impossible rather
than merely unwritten.

It matters most at the moment somebody is building this, because the cheap version is to make
`workspace` an `Option` and read `None` as "not an agent run". That is inference wearing a type,
and it fails in the direction this project has already been burned by: the invalid combinations
stay expressible, and something downstream has to guess what was meant.

**Today this is not true, and it is the largest single thing standing between the tiers above and
the code.** `RunSpec` is agent-shaped throughout: `workspace` is a `WorkspaceSpec` and not an
`Option` (`offload-core/src/run.rs:116`), and its `repo` is a bare `String` (`run.rs:104`), so
*there is no run of any kind that does not name a repository*. Every cheap poller would have to
arrive dressed as an agent run against a repo it never opens. `docs/ROADMAP.md` carries the split
as phase 8's first item and calls it "the largest single edit since capabilities became
instances", which is honest.

### A clock is a program, except when it is a schedule

Two mechanisms, and they made opposite calls on purpose. Getting them confused is the most likely
way to design the wrong thing here.

- **A trigger has no interval.** No cron parser, no HTTP client, no change-detection rule anywhere
  in the orchestrator: the cadence belongs to the nominated program, so `Service::Schedule` is
  something an owner writes (`sh -c 'while sleep 300; do echo tick; done'`) rather than something
  the daemon implements. ADR-0020 §1 refuses the interval field explicitly — it is smaller to write
  and it is a scheduler, which is the "never reimplement" rule broken from the same end.
- **A schedule has ticks**, and pays for them. ADR-0019 §3 derives an occurrence's `RunId` from
  `(schedule, tick)`, so two nodes firing one tick converge on one record instead of racing to
  create two. The caveat is worth carrying verbatim: *a duplicate record merges, and a duplicate
  agent does not.* Creation may converge; placement never may.

### Both directions of control

The fleet drives the agent — that is the substrate, and it is what `offload-node`'s supervisor
does. The other direction is also wanted: **an agent that dispatches work to the fleet**, leaves it
running, and hears about it later.

Nothing new is needed to hand an agent that ability. ADR-0011's `Resource` already projects an MCP
config *and* the permission to call it, so an `offload` server handed to a run is the same
mechanism as any other granted resource. It is also how the phase 7 run-DAG item ("fan out a
refactor across repos, gate on review") stops being a separate subsystem.

Two rules apply to that direction and are easy to skip because the caller is trusted-looking:

- **An agent holding a submit tool is a submitter**, and every submitter rule binds it. ADR-0019 §1
  refuses a submitter-supplied command line — *a repo may say "run my tests", not "give me `sh`"* —
  and ADR-0020 §5 checks `Grant::Submit` when a rule fires rather than only when it is written. An
  agent is a submitter that never gets tired, so these bind harder there, not more loosely.
- **The submitting conversation is not the audience.** A run dispatched by an agent usually
  outlives the turn that dispatched it, so "tell whoever asked" has no referent by the time there
  is something to say. ADR-0026 already answers this — news is addressed by *service* and never by
  route id — but nothing has exercised it for agent-submitted work, and that is where the feedback
  half of this belongs.

## System model

### Assumptions

- Partially synchronous network. Messages delayed, dropped, reordered. Clocks drift.
- Nodes fail by crashing or disappearing (sleep, lid close, network change). Byzantine
  behaviour is out of scope — one person's fleet, all nodes mutually trusted once admitted.
- Membership churns constantly. Churn is the normal case, not the exception.
- Run counts are small: tens concurrently, not millions. This buys the freedom to keep full
  cluster state in memory on every node.
- Agent CLIs are external programs we drive, not libraries we embed. They can be upgraded out
  from under us and their versions differ across the fleet.

### Guarantees we aim for

- **At-most-once execution** per run, enforced by epoch fencing.
- **Eventual placement**: if a node satisfying the constraints is alive and has capacity, the
  run eventually starts there.
- **Bounded recovery**: a run on a failed node is rescheduled within `lease_ttl + detection`,
  targeted under 15 seconds.
- **Resume, not restart**: a migrated run continues from its last checkpointed turn, keeping
  conversation context and uncommitted work.
- **No single point of failure**: any stable node can coordinate.

Not guaranteed: exactly-once side effects, strong consistency of the run registry during a
partition, fair scheduling, or losslessness across a mid-turn crash.

## Layers

```
      ┌────────────────────────────────────────────────┐
      │  offload-cli  (unix socket, JSON)              │
      └────────────────────────┬───────────────────────┘
                               │
      ┌────────────────────────▼───────────────────────┐
      │  offload-node — daemon, wiring, supervision    │
      └──┬─────────┬──────────┬──────────┬─────────┬───┘
         │         │          │          │         │
  ┌──────▼────┐ ┌──▼───────┐ ┌▼────────┐ ┌▼──────┐ ┌▼────────┐
  │  cluster  │ │  sched   │ │  agent  │ │workspc│ │  store  │
  │membership │ │ matching │ │ spawn   │ │worktree│ │ state + │
  │ gossip    │ │ placement│ │ stream  │ │ bundle │ │  blobs  │
  │ failure   │ │ migration│ │ resume  │ │ patch  │ │         │
  └──────┬────┘ └──┬───────┘ └┬────────┘ └┬──────┘ └─────────┘
         │         │          │           │
      ┌──▼─────────▼──────────▼───────────▼────────────┐
      │  offload-proto  (wire messages)                │
      └────────────────────────┬───────────────────────┘
      ┌────────────────────────▼───────────────────────┐
      │  offload-transport (QUIC)                      │
      └────────────────────────────────────────────────┘

      offload-core: types shared by every box above
```

## Membership and failure detection

Gossip-based, SWIM-style:

1. Each node periodically pings a random peer. On no reply it asks `k` peers to probe on its
   behalf — the indirect probe is what stops one bad link from looking like death.
2. Unreachable through any path ⇒ `Suspect`, broadcast via gossip.
3. A suspect that does not refute within `suspicion_timeout` ⇒ `Dead`.
4. Nodes piggyback a **capability digest** on gossip; full capabilities are fetched on digest
   change, so steady-state payloads stay small.

States: `Alive → Suspect → Dead`, plus `Draining` (voluntary, believed immediately — a node
knows when it is leaving).

Discovery: mDNS on LAN plus a static seed list. Phase 4 adds relay/hole-punching so phones on
mobile data participate without a VPN.

## The cluster view

There is no central registry. **Every node keeps a full picture** of the fleet — all nodes,
their capabilities, their owner policies, their workloads, and all runs — and merges gossip
into it. At tens of nodes and tens of runs this is kilobytes, so keeping nodes ignorant of
each other buys nothing.

Merge never needs a judgement call, because every fact has exactly one authoritative writer:

| Fact                                    | Owned by             | Arbitrated by |
| --------------------------------------- | -------------------- | ------------- |
| a node's capabilities, policy, workload | that node            | `incarnation` |
| a node's liveness                       | its peers, refutable | `incarnation` |
| a run's assignment and state            | that run's arbiter   | `epoch`       |

Two deliberate exceptions: locally observed absence history is never overwritten by a remote
(a flappy node must not be able to erase our record of its flapping by bumping its
incarnation), and a holder's own `Running` claim outranks a peer's `Orphaned` suspicion at
equal epoch — the holder knows, the peer is guessing. See ADR-0005.

A practical consequence: `offload ps` on a phone answers locally, without asking anyone.

## Placement: nodes negotiate

Placement is **not** dictated by a central scheduler. Nodes bid for work and a per-run arbiter
grants it. See ADR-0006.

The reason is information. The facts that best predict a good placement are exactly the ones
gossip carries worst: is the repo already cloned here, what is the load *this second*, is the
battery draining, is the user actively typing on this laptop. The node knows all of that for
free, at the moment the decision is made.

```
run goes Pending, visible in every node's view
  │
  ├─ each node evaluates itself, locally:
  │     1. owner policy      — WorkPolicy::admits   → NoBid::Refused
  │     2. eligibility       — Constraint::matches  → NoBid::Ineligible(failing clauses)
  │     3. self-score        — local facts gossip can't carry
  │
  ├─ interested nodes broadcast a Bid after a score-proportional delay
  │     keener bids go out sooner, so the obvious best node usually claims
  │     uncontested and negotiation costs one message
  │
  └─ the run's arbiter grants to the winner, bumping the epoch
        winner() is deterministic (highest score, ties by lowest NodeId),
        so every node agrees without another round trip
```

**Scoring** ranks the eligible:

```
score = stability                     // will this node still be here in an hour
      + workspace_warm                // repo already cloned — the biggest real signal
      + checkpoint_local              // blobs already here, resume needs no transfer
      + cores/memory, per doubling    // agent runs are model-latency bound, not CPU bound
      - load, battery, metered
      - migration_penalty             // moving a healthy run needs a clearly better offer
      - account_pressure              // nodes sharing an account share a rate limit
```

Resources are scored **per doubling**, not per unit. Linear weighting makes a 32-core box
outrank every signal that actually predicts a good placement, which is wrong for a workload
that spends its time waiting on a model.

**The arbiter** for a run is its *home* node — whichever accepted the submission — while that
node is available, otherwise the lowest-id available node. Every node computes the same
successor from the same view, so failover costs no election. Arbitration is per run, so it
spreads across the fleet rather than funnelling through one coordinator.

Split brain during a partition remains possible: two arbiters can each grant the same run.
Epoch fencing means only one commits effects.

**Phase 6 — Raft (`openraft`)** over `Stable` nodes, everything else a learner, if negotiated
arbitration proves insufficient. Deferred deliberately; epoch fencing is required either way.

## Owner policy: capability is not permission

A device's owner decides what it will do, separately from what it can do. This is what makes
a phone a safe worker rather than a liability.

```
accept:              never | when_charging | always
max_concurrent_runs: 1
min_battery_percent: 40
allow_metered:       false
allowed_agents:      any
```

Defaults are per device class — a phone opts in **while charging**, at one run, above 40%
battery, off metered data. A laptop takes two runs and stops at 20% battery.

Policy is checked by the node itself, before eligibility, and its refusals are reported
distinctly: "phone declined: not charging" and "phone ineligible: no rust toolchain" are
different sentences, and conflating them makes `offload explain` useless.

## Capabilities: what actually matters for agents

Beyond the usual resource facts, a node advertises:

```
agents:     claude-code 2.1.4, authenticated, models: [opus-5, sonnet-5], max_concurrent: 2
toolchains: rust 1.82, node 22.3, python 3.12, docker
tools:      gh (authed), git 2.45, ffmpeg
network:    unmetered, egress unrestricted
account:    <opaque account fingerprint, for per-account rate limiting>
```

The `account` fingerprint matters more than it looks: three nodes authenticated to the *same*
agent account share one rate limit, so concurrency has to be capped per account across the
fleet, not just per node.

**Credentials are never part of an assignment.** A run is placed on a node that already holds
its own auth. Nothing in the protocol ships a token to a peer.

## Run lifecycle

```
                 ┌─────────┐
                 │ Pending │◄─────────────────────────────────┐
                 └────┬────┘                                  │
                      │ arbiter grants to the winning bid     │
                      │ (epoch + 1)                           │
                 ┌────▼─────┐                                 │
                 │ Assigned │                                 │
                 └────┬─────┘                                 │
                      │ workspace ready, agent up             │
                 ┌────▼────┐                                  │
            ┌────┤ Running │◄────────┐ reclaim, same epoch    │
            │    └──┬───┬──┘         │ no migration, no loss  │
            │       │   │            │                        │
            │       │   │ contact lost                        │
            │       │  ┌▼────────────┴┐                       │
            │       │  │   Orphaned   ├── reassign ───────────┘
            │       │  └──────┬───────┘   (epoch + 1)
            │       │         └── abandon (Pinned only) ──┐
            │       │ drain / preemption requested        │
   complete │  ┌────▼─────────┐                           │
            │  │ Checkpointing│ finish turn, capture      │
            │  └────┬─────────┘                           │
            │       └──► Pending, checkpoint attached     │
       ┌────▼─────┐                                       │
       │Completed │   Failed ◄─────────────────────────────┘
       └──────────┘   Cancelled
```

Every entry into `Assigned` bumps the epoch, and so does releasing a run at a checkpoint —
which both invalidates the departing holder immediately and keeps `(epoch, progress)`
monotonic, so gossiped run states merge unambiguously.

`Orphaned` grants **no authority**: the last holder cannot act under it. That is load-bearing.
If it ever returned a live lease, an out-of-contact node could complete or commit to a run the
arbiter is about to move elsewhere.

## When a node drops off

Losing contact is an observation, not a decision. Most disappearances mean nothing — a laptop
suspends for eight seconds, Wi-Fi hands over, a phone's radio sleeps — and the node comes back
with its agent still running and its tree intact. Reassigning on every one of those turns a
non-event into a lost turn, a re-cloned repo, and on a flappy network a run that ping-pongs
around the fleet doing nothing but migrating.

So a lost holder moves the run to `Orphaned` and the hold-down policy decides. The wait is
computed per case rather than configured as one number:

| Signal                          | Effect on the wait | Why                                        |
| ------------------------------- | ------------------ | ------------------------------------------ |
| Holder is `Draining`/`Departed` | go now             | it told us; waiting is pointless            |
| Holder usually returns quickly  | wait ~150% of that | a node that always returns in 90s earns it  |
| Holder confirmed `Dead`         | shrink to ~40%     | stronger evidence than mere silence         |
| Run has no checkpoint yet       | double it          | moving now discards everything done so far  |
| Run is `Idempotent`             | halve it           | restarting is cheap                         |
| Run has moved before            | add per attempt    | anti-thrash                                 |
| Nowhere else to put it          | wait indefinitely  | patience is free with no alternative        |
| Holder answering again          | brief hold         | let it reclaim rather than racing it        |

All clamped to `[15s, 10min]`. The floor exists specifically to absorb suspends and handovers.

Absence history is an exponential moving average kept per node, weighted ¼ toward the newest
sample, so the system adapts to each device's actual behaviour rather than to a guess baked
into config.

Decisions carry their reason — `Hold { until, reason }` rather than a bare bool — so
`offload ps` can say *"waiting 40s more for laptop-2, which usually returns in 90s"* instead
of looking hung. See ADR-0007.

## Migration

A checkpoint is three things:

1. **Transcript** — the agent's session state, plus its native session id for `--resume`.
2. **Workspace bundle** — a git bundle of the run's branch, plus a patch of uncommitted
   changes, plus the list of untracked files that matter.
3. **Run metadata** — cwd, env allowlist, turn count, agent version, model.

All content-addressed by BLAKE3 and stored as blobs. Migration is therefore: *"fetch these
three hashes and resume"*.

```
node        marks itself Draining, gossips it
agent       current turn is allowed to finish (bounded by drain deadline)
workspace   git bundle + dirty patch captured, blobs uploaded
runtime     lease released
coordinator observes Draining + released lease, reassigns to next-best node
new node    fetches blobs, materialises worktree, resumes agent from session id
            epoch bumped; old node's writes now rejected
old node    exits once handoff confirmed or deadline passes
```

Ungraceful loss is the same minus cooperation: the lease expires and the run resumes from its
last *durable* checkpoint. So **checkpoint cadence determines how much work a crash costs** —
by default, one turn.

The hard constraint is that checkpoints only happen at turn boundaries (ADR-0004). An agent
mid-tool-call has state in the tool — a half-written file, an open subprocess — that the
transcript does not capture. A drain deadline that expires mid-turn loses that turn; it does
not snapshot anyway.

**Version skew:** an agent's resume format is its own business and is not guaranteed portable
across agent versions. Nodes advertise their agent version; migrating a run to a node with an
older agent is disallowed at the eligibility level, not discovered at resume time.

## Content-addressed blobs

Transcripts, bundles, patches, and run outputs are blobs keyed by BLAKE3. This gives dedup
(the same repo bundle across many runs), integrity, and makes migration a hash-fetch.

A node needing a blob **asks** for it — availability is not gossiped, because the set changes
with every checkpoint and would become the most volatile thing in the view, to answer a
question that arises only when a node has died. Integrity comes from the hash rather than from
the sender, so a blob may be fetched from any member. **A checkpoint is not durable until a
second node holds it**, so capture pushes to a peer that could plausibly take the run over.
See ADR-0016.

Small blobs (< 64 KiB) travel inline in the assignment message; larger ones go through the
store.

## Security

All nodes are mutually trusted after admission, so the surface is join-time, transport, and
what the agent itself does.

- Node identity is an ed25519 keypair; `NodeId` derives from the public key, so it is not
  spoofable by choosing a name.
- Transport is QUIC with TLS 1.3, certificates pinned to node identity.
- Joining requires a pre-shared cluster secret. Phase 4 adds enrolment so a new device is
  admitted by an already-admitted one without hand-copying secrets.
- **Agent credentials never leave their node.** Auth is a node capability; assignments carry
  run specs and blob hashes, never tokens.
- Agents run with whatever privileges `offloadd` has and, by design, take autonomous actions
  on repos. Treat the permission mode in a run spec as a real security control and default it
  conservatively. Sandboxing is phase 6 — assume it absent until then, and do not point this
  at repos you would not let an agent write to unattended.
