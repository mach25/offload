# ADR-0049: The owner's policy reaches the one door that opens itself

**Status:** accepted · 2026-08-30 · completes ADR-0046 and ADR-0047, extends ADR-0043's handover
to a second cause · no wire change, no schema change

## Context

Three sessions have now put the owner's work policy on a door that starts an agent: the
no-cluster submission arm (ADR-0046), `offload resume` (ADR-0047), and — for the drain and the
revocation rather than the policy — the recovery tick (ADR-0034, ADR-0044). The tick was the one
left, and it is the only door **nobody types at**: it opens by itself, unattended, at 02:00, which
is what it is for.

`Circumstances` carried `departing` and `revoked`, each added when its absence was measured, and
did not carry the owner's standing answer. So a node that would refuse a submission and refuse a
resume still restarted its own failed runs.

### Measured

One daemon, a fake `nmcli` on its `PATH` so the link becomes metered under a running process
(ADR-0048's staging), and a fake agent that fails on its first leg:

```
16:21:45  run starts, link free
16:22:06  what this device is has changed          (metered)
16:22:33  run failed; noted who was watching       attendance=unattended
16:23:06  nobody was watching; resuming it         resume=1
16:23:06  spawning claude code
```

Between 16:22:06 and 16:23:06 the node reported `accepting no — network is metered and policy
disallows it`, refused a submission at its own socket, and refused an `offload resume` of that very
run. Sixty seconds after it began refusing everything a person could ask of it, it started the
agent on its own.

### The order this was found in is the point

Seven doors, one gate, and each was found separately:

1. the bid, 2. the grant, 3. the no-cluster submission arm's drain clause, 4. the recovery tick's
`departing`, 5. the recovery tick's `revoked`, 6. the no-cluster arm's policy clause (ADR-0046)
and `offload resume` (ADR-0047), 7. this one.

Every fix was measured, correct and written down. The thing that kept being missed is not a
clause; it is that **a gate has as many doors as there are ways to start an agent, and the last
one to be found is always the one that opens without being asked.**

## Decision

### 1. `Circumstances` gains `hosting: Hosting`

```rust
pub enum Hosting { Allowed, RefusedByOwner }
```

A type rather than a fourth `bool`, for the reason `Standing` is one and for the reason
`Circumstances` itself exists: this struct was created because a boolean among small integers is a
call site nobody can read, and adding a fourth boolean to it would be undoing that.

It differs from its two neighbours in where it comes from. `departing` and `revoked` are latches
something set; this is re-answered every pass from `WorkPolicy::permits` over the **live**
capabilities (ADR-0048) — so a battery that drains or a link that becomes metered changes it under
a running daemon, which is exactly the case that made the gap reachable at all. It is a parameter
to `recover_failed_runs` rather than a field on the supervisor or a `Config` lookup inside it,
because a value that changes per pass must be answered per pass, by the caller that already holds
the probe everything else is deciding with.

### 2. It behaves like `departing`, not like `revoked`

A node that will not host is still a node the fleet can hear. So `Resume` and `Wait` become
`Recovery::LetGo` and the run goes back to the fleet — ADR-0043's mechanism unchanged, with its
`Checkpoint::is_durable` guard, because offering a run whose conversation exists nowhere else
leaves it `Pending` behind an unreachable checkpoint, which is worse than the `Failed` it came
from.

`revoked` stays above both: letting go is an offer to a fleet a revoked node no longer has.

Rejected: escalating instead of handing over, on the grounds that a node under its battery floor
is *staying* and could pick the run up again when it is plugged in. That is true and it is the
wrong trade. A laptop on battery overnight would sit on a failed run while a desktop is idle,
which is the fleet failing at the one thing it exists for; and `Pending` is a better place than
`Failed` for exactly that reason — this node can bid for it again the moment the policy allows,
where nothing ever offers a `Failed` run.

### 3. A third cause needs a third sentence

`Escalation::OwnerWillNotHost`, said where the fleet could not have started it anyway:

> this node's owner does not allow it to host runs, and its conversation exists nowhere else

Not a reworded `NoCopyElsewhere`. That variant says *this node is leaving*, and telling somebody
their laptop is leaving when it is sitting there with 15% of a battery is the wrong cause
confidently stated — which is precisely what ADR-0043 removed `NodeIsDeparting` for. The two
states differ in what the operator does next: wait for the node to come back, or plug it in.

Where both are true, `departing` wins and says so. A node that is leaving is leaving whatever else
its owner said — the precedence `GivenUp::LetGo` already has over `Parked`.

### 4. `offload explain` asks the same question the same way

`NodeStanding` gains the field, filled from `server::hosting_allowed`, which is
`owner_policy_refusal` — the same function `hosting_refusal` uses for the two doors a person
types at, over the same live handle the `accepting` line reads. One fact, one place, four
readers. A second way of computing it here is how a report comes to disagree with the decision it
describes.

The `LetGo` line in `explain` now names which of the two causes it is, for §3's reason.

## Consequences

- A node that says `accepting no` for a policy reason starts no agent by any route: bid, grant,
  submission, resume, or its own recovery tick.
- Measured after, both branches. On the fleet of one that produced the symptom the run stays
  `failed`, nothing spawns, and `offload explain` says `left for a person: this node's owner does
  not allow it to host runs, and its conversation exists nowhere else`. On two daemons with the
  checkpoint replicated, the run reaches the peer 412ms after the handover and carries on in the
  same conversation.
- **The handover branch, walked**, once the machine went idle — and it found the third place this
  ADR's own sentence lives. Two daemons, the run on `alpha` with its checkpoint replicated to
  `bravo`, `alpha`'s link turned metered underneath it:

  ```
  16:48:49.536  handing the failed run back to the fleet
                  reason="this node's owner does not allow it to host runs"
  16:48:49.948  spawning claude code   session=fake-session-1   (on bravo)
  ```

  412ms, same conversation, turn 12 to turn 16 on the other machine — and `alpha` never started an
  agent. On the first pass that log line read **"this node is leaving; handing the failed run back
  to the fleet"**, about a laptop sitting right there on a metered link. §3 is about exactly that
  mistake and fixed the two places it was looked for — the escalation and `offload explain` — and
  missed the `tracing::info!` a few lines from the transition. **The count was wrong in the same
  way the door count keeps being wrong**: a sentence has as many places as it has callers, and the
  one found last is the one nobody was reading. It names the cause now.
- Two tests, both properties rather than examples. The first sweeps every shape of failed run ×
  attendance × standing × resumes × turns × clock against `RefusedByOwner` and asserts that
  `Resume` and `Wait` never appear — the same sweep `departing` and `revoked` each have, because
  the guarantee is a property of one `match` arm. It also asserts the node never claims to be
  leaving. The second pins the two outcomes either side of `fleet_could_start`, and the precedence
  when more than one of the three is true.
- No wire change and no schema change: `Escalation` is in-memory and rendered by `offload explain`;
  `Circumstances` never leaves the node that builds it.

## What this deliberately leaves

**A run handed back for a policy reason may come straight back to the same node — measured, and
there is nothing here.** This was written as a residual and then walked, on two daemons where
*nobody* could take the run: `alpha` metered, and `bravo` enrolled without `host-runs`, which
holds a replica and can never win it. Two minutes:

```
17:04:16.244  still nobody for it  refusals=2  offering=LetGo { by: alpha }
17:04:46.406  …then every 30.2 seconds, exactly `REASSIGN_RETRY`
```

One round per thirty seconds — 120 an hour, ~960 over an eight-hour night, each a canvass of the
live fleet — nothing at INFO, and **the epoch never moves**: at six minutes `explain` still
reports epoch 2, the one the handover spent. `may_retry` already rate-limits this, with the same
backoff a reassignment uses and for the same reason: a fleet that had no room a second ago still
has none.

So no lever is needed, and the shape of the check is the part worth keeping: **count the rounds,
then check the epoch.** A retry loop that is cheap in messages and expensive in epochs is the one
that would have mattered, and a log-line count would not have shown it.

**Nothing re-examines a run that was escalated before the policy changed.** `Recovering::decided`
is remembered, and a run left for a person when the link was metered stays left for a person when
it is not — a person resumes it, which is now allowed again. Re-deciding on every change would
mean an escalation that is not a decision but a mood, and the rule that a fresh retry budget must
not be a side effect points the same way.
