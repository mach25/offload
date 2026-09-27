# ADR-0046: A node's standing refusal is not a queue, and every path has to ask it

**Status:** accepted · 2026-08-30 · repairs an ordering inside ADR-0011's control and a missing
clause on ADR-0014's no-cluster path · no wire change, no schema change

## Context

Two findings that look unrelated and are the same sentence twice: **a "never" was being reported
as a "not yet", and a path that should have asked for one asked for neither.**

### 1. The fleet of one asks two of a bid's questions

`server::place` has two arms. With peers, the fleet decides and a bid round answers. Without, the
comment says:

> Without, "here" is the only answer there has ever been — so **the two questions a bid would have
> asked** are asked right here instead, in `Host::evaluate`'s order and for its reasons.

`Host::evaluate` asks three things: `is_draining()`, `host_runs_refusal()`, and
`offload_core::bid::evaluate`, which is where `WorkPolicy::admits` lives. The arm asks the first
two. The comment's own history records the first one being added after exactly this was measured:

> The drain half was missing, and a fleet of one is the case where it is the *whole* feature: there
> is no bid to refuse and no grant to decline, so `offload drain` set a flag that nothing on this
> path read. Measured — drained, `offload status` saying `accepting no`, and the next submission
> accepted and started anyway.

Measured again, word for word, for the clause that is still missing — a daemon with
`metered = "yes"` and `[cluster] enabled = false`:

```
$ offload status
accepting   no — network is metered and policy disallows it
$ offload run --repo … "did this run?"
run 01a052e193fd
$ offload ps --all
RUN            STATE           TURNS  …  PROMPT
01a052e193fd   completed           1  …  did this run?
```

The same command against the same config with a cluster — `offload init`, a restart, a one-node
bid round — is refused:

```
Error: no node will take this run
  fedora       network is metered and policy disallows it
```

So today the answer to "does the owner's policy bind a submission typed at this machine" is
**whether the node happens to be in a fleet**. Nobody decided that, and it is not defensible
either way round.

`AcceptWork::Never`'s own doc comment settles which way it should go:

> `Never` — Control plane only: submit and observe runs, **never host them**.

Not "never host what the fleet sends"; never host them. The struct's own heading says the same:
*"Owner policy: may this node take work at all?"*

### 2. …and `allowed_agents` was checked after capacity

`admits` ran, in order: accept / battery / metered → **capacity** → pressure → `allowed_agents` →
per-agent concurrency → account.

`bid::evaluate` draws a line through that list in its own comment, and the line is load-bearing:

> Being *full* is the one refusal that is not a no. It is this node's own queue, it empties by
> itself… So a full node offers anyway and says when (ADR-0006). **Every other refusal here is a
> condition of the device, not a queue.**

A full node therefore *bids and commits*. `allowed_agents` is not a queue — it is the owner saying
what kind of work this machine is for, which is the one policy knob a device class cannot express
(ADR-0011) — and it was being reached only after the queue clause had already returned. Measured:

```
admits(desktop, allowed_agents = ["something-else"], claude-code,
       Normal, holding 1 of max 1)
  → Err(AtCapacity { running: 1, max: 1 })
```

Read rather than measured, and named as such: `bid::evaluate` maps `AtCapacity` to `Some(running)`
and returns a `Bid`; `NodeHost::accept` re-checks only `is_draining`; `start_held_runs` starts the
run when a slot frees. So a node whose owner forbade this agent, and which happened to be full when
it was asked, would accept the run and later start it. That chain wants a walk before it is called
proven — but the ordering is wrong whether or not the walk finds the whole chain intact, because
the refusal a caller receives already misdescribes the node.

## Decision

### 1. Split the owner's standing answer out of the load question

```rust
pub fn permits(&self, caps: &Capabilities, agent: &AgentKind) -> Result<(), Refusal>
```

Four clauses, and they are exactly the ones that do not depend on what the node is currently doing:
`accept`, `min_battery_percent`, `metered`, `allowed_agents`. Each is a fact about the device or a
standing instruction from its owner. None of them empties by itself.

`admits` calls `permits` first and then keeps everything else in its existing order: capacity,
pressure, per-agent concurrency, account ceiling. One rule, one place; the second entry point reads
the same code rather than a copy, which is the standing requirement here for anything a report and
a decision both consult.

### 2. That hoists `allowed_agents` above capacity, deliberately

A standing refusal must be reported before a queue, because the fleet treats the two differently:
one is a commitment, the other is a no. Reporting a "never" as a "not yet" is not a cosmetic
mis-ordering — it is what makes the node promise the work.

The general form, worth keeping when the next clause is added: **inside `admits`, order by whether
the refusal can go away on its own.** Everything that cannot goes in `permits`.

### 3. The no-cluster submission path asks `permits`, not `admits`

`server::place`'s `None` arm gains the third question, after the drain and the grant, in
`Host::evaluate`'s order.

`permits` rather than `admits`, and the difference is not laziness:

* **The queue half is already answered there, better.** `Supervisor::submit` calls
  `Room::for_one_more`, which sees the device-wide ledger (ADR-0013) and the account's rate limit
  as well as this node's own count. Calling `admits` too would ask a weaker version of the same
  question first and report it in different words.
* **Being full means something different on a fleet of one.** ADR-0006's commit-rather-than-decline
  exists because a bid round has somewhere to put the run in the meantime. With no round, at
  capacity is a refusal — which is what this path already does, and what `docs/DEMO.md` records.

So the two arms end up asking the same standing question and answering the queue question in the
way that suits each. That is the invariant to hold: **`permits` is asked on every path that can
start a run; the queue clause is each path's own.**

## Consequences

- A node saying `accepting no` for a policy reason now refuses a submission typed at its own
  socket, with the same sentence `offload status` prints. The report and the behaviour agree, on
  both arms, for the first time.
- `accept = "never"` means what its doc comment has always said. On a machine with no fleet, that
  is a behaviour change: `offloadd` on a laptop configured as control-plane-only will no longer run
  an agent locally. It is the configured behaviour finally taking effect, and it is reachable only
  by having written that line.
- **Some devices change behaviour on their defaults, and it is worth stating rather than
  reassuring.** `WorkPolicy::for_class` is not uniform: a desktop, server or VM is `Always` with no
  battery floor and `allow_metered`, so nothing about it changes — but a **laptop** carries
  `min_battery_percent: 20` and a **phone or tablet** carries `WhenCharging` and a floor of 40. So
  a laptop under 20% on battery, and a phone that is not charging, will now refuse a submission
  typed at their own keyboard where they used to run it. That is the configured policy taking
  effect on a path that was not consulting it — the cluster path has always refused exactly there —
  but it is a change somebody could meet without having edited anything, which is the kind this
  project says out loud. The lever is the owner's: `accept`, the floor, or plugging the thing in.
- `metered` is `Unknown` until somebody says so (ADR-0045) and `Unknown` does not refuse, so
  nothing changes for that clause on any device until an owner nominates or NetworkManager states
  it. `allowed_agents` defaults to `None` everywhere.
- A refusal that cannot go away on its own is reported ahead of one that can, so a node that is
  both full and forbidden an agent says the thing that will still be true tomorrow.
- No wire change and no schema change: `WorkPolicy` is unchanged as a type, and `permits` is a new
  entry point into rules that already gossiped.

## What this deliberately leaves

**Pressure stays out of `permits`, and out of the no-cluster path.** `UnderPressure` is a load
average, it clears on its own, and `bid::evaluate` already treats it as neither a queue nor a no —
it defers if the run has slack and otherwise sends the run elsewhere. On a fleet of one there is no
elsewhere, and refusing a submission because the machine is briefly busy would be a worse answer
than running it. Left as it is on both arms.

**Three of a bid's other questions are still unasked on the no-cluster path**, and each is left for
a reason rather than by omission:

* `repo_available` — on a fleet of one the repo is either here or the run fails at workspace
  preparation, loudly, on the machine the person is sitting at.
* the run's `constraint` — every submission is built with `Constraint::agent_ready(ClaudeCode,
  None)` and `host_runs_refusal` already covers the grant; nothing user-written can reach the tree
  today, as the correction to ADR-0045 records.
* the checkpoint's `agent_version` — this matters on `resume`, not on submission, and the
  no-cluster resume path is a separate question this ADR does not open.

Each of those would be a *fourth* question on this arm and none has a measured failure behind it.
The rule this ADR would apply to them is already written down: the arm is supposed to ask what a
bid asks, so the burden is on leaving one out.

**No override flag.** The obvious escape hatch is `offload run --here` or `--force` — "I know the
policy, run it anyway" — and it is not built, because the alternative reading of this whole ADR is
that a person at the keyboard *is* the override and no flag is needed. Deciding that by adding a
flag would settle it quietly and in the direction that costs a new piece of CLI surface. What a
laptop at 15% actually needs today is `[policy] min_battery_percent = 0` and a restart, which is
the owner changing their own standing instruction — the same lever, said once rather than per run.
If the friction turns out to be real, the flag is the answer and this paragraph is where to start;
the thing to bring to it is a count of how often somebody hit the refusal and meant it.

**The commit-a-forbidden-agent chain is not walked.** §2's measurement is the refusal `admits`
returns; the rest — bid, commit, start — is read. The ordering is fixed because the refusal itself
is wrong, and the walk is worth doing to find out whether the run really does start, which is the
difference between a misleading report and a node running work its owner forbade.
