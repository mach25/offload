# ADR-0007: A node dropping off is an observation, not a decision

**Status:** accepted · 2026-07-25 · extended by ADR-0013: the run's `Urgency` becomes an
input to `grace_for` and `decide_reassignment`. An added input, not a changed rule — absence
history still says how flaky a node is, urgency says how much of that this run can afford.

## Context

Devices vanish constantly, and most of the time it means nothing. A laptop suspends for
eight seconds. Wi-Fi hands over between access points. A phone's radio sleeps. A VM's host
pauses it briefly. In every one of those cases the node comes back with its agent still
running and its working tree intact.

The naive design — lease expires, reassign — turns each of those non-events into a real cost:
a migration, a lost turn, a re-cloned repo, and on a flappy network, a run that ping-pongs
around the fleet doing nothing but migrating.

But the opposite failure is worse. Waiting indefinitely for a laptop that is in a bag at the
airport means the run never progresses, which is precisely the situation the project exists
to fix.

So the question is not *whether* to move work, it's *how long to wait first* — and that
answer is different for every combination of node and run.

## Decision

Losing contact moves a run to **`Orphaned`**, a distinct state that means "the holder is out
of contact and we have not yet decided anything". It is not `Pending`, and it grants no
authority: the old holder cannot act under it, and no new node can be assigned from it
without a decision.

From `Orphaned`, three things can happen:

- **Reclaim.** The original holder returns and resumes at the *same epoch*, with no
  migration, no lost turn, and no new attempt recorded. The cheapest possible outcome, and
  the most common one.
- **Reassign.** The hold-down expires, and the run is granted to someone else at a new epoch.
- **Abandon.** Only for `Pinned` runs, which have nowhere to go.

The hold-down is computed per case, not configured as one number:

| Signal | Effect on the wait | Why |
| --- | --- | --- |
| Holder is `Draining`/`Departed` | reassign immediately | it told us; waiting is pointless |
| Holder's typical absence is short | wait ~150% of it | a node that always returns in 90s has earned patience |
| Holder is confirmed `Dead` | shrink to ~40% | stronger evidence than mere silence |
| Run has no checkpoint yet | double it | moving now throws away *everything* the agent has done |
| Run is `Idempotent` | halve it | restarting is cheap, so don't wait around |
| Run has moved before | add per attempt | anti-thrash; a ping-ponging run slows itself down |
| No eligible node to move it to | wait indefinitely | patience is free when there is no alternative |
| Holder is answering again | brief hold | let it reclaim rather than racing it |

All clamped to `[min_grace, max_grace]`, defaulting to 15s and 10 minutes. The 15s floor
exists specifically to absorb suspends and Wi-Fi handovers.

`decide_reassignment` is a pure function of `(run, observation, alternatives, now, policy)`,
returning a decision *with its reason attached* — `Hold { until, reason }` rather than a bare
bool, so `offload ps` can say "waiting 40s more for laptop-2, which usually returns in 90s"
instead of leaving the user to guess.

Absence history is an exponential moving average maintained locally per node, weighted 1/4
toward the newest sample so an unusual outage is forgotten after a few normal ones.

## Consequences

Good: the common case — brief disappearance — costs nothing at all. The system adapts to each
device's actual behaviour rather than to a guess baked into config, and it does so from data
it collects for free. Because the decision is pure and reasoned, it is both testable and
explainable, and those turn out to be the same property.

Bad:

- **A run can sit `Orphaned` for up to `max_grace` doing nothing.** For a node that is never
  coming back, that is pure latency. Tuning `max_grace` down trades this against migration
  churn, and there is no setting that is right for every fleet.
- **The EMA needs history to be useful.** A node's first few absences get the default, which
  may be badly wrong in either direction.
- **More states, more transitions, more to get wrong.** `Orphaned` granting no authority is
  load-bearing: if it ever returns a live lease, the old holder can act on a run we are about
  to move, and the fencing story collapses.
- Absence history is per-node-observer and not gossiped, so different nodes may compute
  different graces for the same holder. Harmless — only the arbiter's decision is acted on —
  but confusing when debugging across two machines' logs.

## Alternatives

**Reassign on lease expiry, no grace.** What most schedulers do, and correct for stateless
short tasks. Rejected: agent runs are long and stateful, so migration is expensive and
suspends are common. This is the specific behaviour the project exists to avoid.

**One fixed grace period.** Much simpler, no history to maintain. Rejected because the right
value differs by an order of magnitude between a VM and a phone, and choosing one number
means being wrong for most of the fleet — but note this is what the policy degrades to when
no history exists, so it is the floor rather than a rejected idea.

**Wait for explicit confirmation of death.** Safest possible. Rejected: a crashed node never
confirms anything, so this is indistinguishable from waiting forever.
