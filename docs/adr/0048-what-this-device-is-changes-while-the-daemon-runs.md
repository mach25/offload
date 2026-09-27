# ADR-0048: What this device *is* changes while the daemon runs, and every reader has to see it

**Status:** accepted · 2026-08-30 · repairs the fact ADR-0046 and ADR-0047 decide with · no wire
change, no schema change

## Context

ADR-0046 gave the no-cluster submission arm the owner's policy. ADR-0047 gave it to
`offload resume`. Both read `Ctx::capabilities` — and that was an `Arc<Capabilities>` built once,
in `main`, and never replaced for the life of the process.

The comment above it had been there since phase 1 and was still describing the future:

> Probe once at startup. Capabilities do change — battery drains, agents get upgraded — and
> **phase 3 will re-probe on a schedule to keep gossip honest. For a single node with no one to
> tell, once is enough.**

Phase 3 built exactly what that sentence promised, and no more. The re-probe handed its result to
`Cluster::set_capabilities` and to nothing else, from inside the gossip loop — the loop a fleet of
one never enters. The second sentence was true right up until this session, when two ADRs gave a
single node's own doors something to decide with.

### Measured

One daemon, a fleet of one *with* a cluster, and `nmcli` replaced on its `PATH` by a script that
reads its answer from a file, so the link becomes metered underneath a running process with
nothing about the machine's real network touched. Thirteen seconds after the flip the daemon
logged `capabilities changed; gossiping them`. Then, three commands, seconds apart:

```
$ offload status
accepting   yes
$ offload run --repo … "new work?"
Error: no node will take this run
  solo         network is metered and policy disallows it
$ offload resume 01a053607c0a
resumed 01a053607c0a
$ offload ps --all
01a053607c0a   running             4  …  count to twelve
```

One fact, three readers. The bid round reads the cluster's copy and refuses. The `accepting` line
and the resume door read the snapshot, say yes, and start an agent on the link the owner is paying
for.

And on a **fleet of one with no cluster**, which is how phases 1 and 2 ran and how most of this
still runs, there is no re-probe at all. Measured on the build before this ADR: sixty seconds after
the flip, `offload probe` said `network metered — bytes cost money here`, `offload status` said
`accepting yes`, and the submission ran. ADR-0046's whole point — that the owner's policy binds a
submission typed at this machine — was being decided from a fact the daemon stopped asking about
at startup.

### Why it hid

**A snapshot is correct until somebody decides with it.** For five phases nothing local read these
capabilities to make a decision: `offload status` printed them, and printing a stale battery
percentage is a small lie. The two ADRs that turned them into a gate were written this session,
one after the other, and neither asked *how fresh is the fact I am now deciding with* — which is
the same shape as ADR-0047's own lesson one level down. Counting the doors is not enough if the
thing behind the door is stale.

The second half is a rule already written down: **a node's own state needs tending whether or not
there is a fleet.** `tend_own_runs` exists because that was got wrong for leases and recovery;
`Mesh::drain`'s flag was got wrong the same way; and the re-probe was the third instance, sitting
in the gossip loop where a lone laptop never reaches it.

## Decision

### 1. One live handle, `deliver::Current`, and the doors read through it

```rust
pub struct Current(Mutex<Arc<Capabilities>>);
impl Current {
    pub fn now(&self) -> Arc<Capabilities>;
    pub fn refresh(&self, capabilities: Capabilities) -> bool;
}
```

`Ctx::capabilities` is `Arc<Current>`, and every reader — `status`, `hosting_refusal`,
`local_view`, the capacity questions — calls `now()`. A `Mutex<Arc<..>>` rather than a lock held
across the read: callers want a whole consistent `Capabilities`, cloning the `Arc` is cheap, and
the probe never waits on a request handler.

**One read per decision.** Where a site needed both the device class and the capabilities, it now
takes one `now()` and uses it twice — otherwise a re-probe landing between the two calls would
answer with half of each. That is not hypothetical here: the class is what selects the policy the
capabilities are then judged against.

### 2. The re-probe is its own task, spawned whether or not there is a fleet

It moved out of the gossip loop, which is the loop that does not exist on a fleet of one. Cadence
unchanged and still derived from the same two numbers (`probe_interval_ms` × `REPROBE_EVERY`), so
a walk that shortens one knob still gets both.

### 3. One writer, one pass, and the fleet's copy is still the cluster's business

`refresh` and `set_capabilities` are called from the same probe in the same statement, so the
daemon's copy and the fleet's cannot answer differently. The *gossip* decision stays where it was:
`Cluster::set_capabilities` compares against the view and decides whether the change deserves an
incarnation. `refresh`'s own boolean is for a log line — so a node with nobody to tell still says
`what this device is has changed` once — and must not become a second answer to the gossip
question.

Rejected: making the cluster the single owner and having `Ctx` read through it. It is the obvious
de-duplication and it reintroduces the bug, because a fleet of one has no cluster to read.

## Consequences

- The report and the doors agree, live. Measured after the fix, same staging: `accepting no —
  network is metered and policy disallows it`, the submission refused, the resume refused, the run
  left `pending`. And on the fleet of one with no cluster, where before there was no re-probe at
  all, the same three answers, from a loop that now runs.
- ADR-0046 and ADR-0047 mean what they say for the two clauses that are *probed* rather than
  nominated. This is worth stating precisely, because it is the whole reachable surface: `accept`
  and `allowed_agents` come from the config and cannot change without a restart; the **battery
  floor** and **metered** come from the probe and change under a running daemon. Those two were
  the only ways either ADR's gate could have been reached by a change of circumstance, and both
  were reading a startup snapshot.
- A laptop that goes onto battery and under its floor now refuses at its own keyboard, which is
  ADR-0046's stated consequence actually taking effect rather than only being written down.
- One test, and it asserts the property rather than the plumbing: after a refresh the door and the
  report say the *same* thing and it is the new thing. The equality holds whatever else is true of
  the machine the test runs on, because `admits` asks `permits` first (ADR-0046) — a standing
  clause is what the report names even when the fixture is also full or under load.
- No wire change and no schema change: `Capabilities` is unchanged, and what gossips is what the
  cluster was already being handed.

## What this deliberately leaves

**The cadence is still thirty seconds, and nothing probes on demand.** A request handler that
probed would shell out to the agent binary and to `nmcli` on every `offload status`, which is the
reason this was ever a snapshot. So there is a window — up to one cadence — where the daemon acts
on a link that has just changed. It is bounded, it is the same window the fleet has always had for
gossip, and the alternative is a status command that takes a second. If it ever needs to be
tighter, the lever is the cadence and not the architecture.

**`offload probe` still probes fresh, and can therefore disagree with `offload status` for up to
a cadence.** That is honest — one command asks the machine, the other asks the daemon — and it is
what made this measurable in the first place. Worth keeping in mind before treating a difference
between them as a bug.

**The recovery tick still does not ask the owner's work policy** (ADR-0047's residual, unchanged
and now unblocked). It was going to be this session's work; the freshness defect is upstream of it,
because the only clause a tick could usefully consult — the battery floor — is one of the two this
ADR just made live. The shape is still a standing fact in `Circumstances` beside `departing`,
probably answering `LetGo`, with `decide_recovery`'s exhaustive sweep to re-run.
