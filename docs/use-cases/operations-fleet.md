# Work that is not a repository, on devices that cooperate

Status: **exploration.** Nothing here is decided and nothing is built. The generic mechanism this
was looking for turned out to exist already, as an accepted and unbuilt ADR; most of what follows
is about what that ADR does and does not reach.

The prompting case is a person who manages ~40 WooCommerce shops and wants the fleet to watch
them, keep them correct, and advise on updates. They did not ask for a repository to be involved.
That is the whole finding: **nothing in the use case is repo-shaped, and everything in the
implementation is.** `RunSpec.workspace` is a `WorkspaceSpec` and not an `Option`
(`offload-core/src/run.rs:116`); `WorkspaceSpec.repo` is a bare `String`. There is no run of any
kind, today, that does not name a repository.

## The two workloads

They arrived together and they are not the same shape. Most of the design pressure is in the gap
between them, so they are named separately throughout.

**A — continuous health.** Each shop exposes a health endpoint. Poll it every few minutes; when
something is wrong, look at it: check billing, check shipping, reconcile order counts, apply a
small correction or tell a person.

**B — update review.** Periodically: what plugin updates are available, read the changelogs,
optionally inspect the diff, recommend — hold, apply, apply-and-verify.

| | A: health | B: updates |
| --- | --- | --- |
| Cadence | minutes | daily or weekly |
| Most firings produce | nothing | a recommendation |
| Urgency | a down shop is urgent | never urgent |
| Needs an agent at all? | **mostly no** — it is a comparison | **yes** — judging a changelog is the job |
| Model | the cheapest that works | not the cheapest |
| Fan-out | 40 independent questions | **one analysis, then 40 cheap verdicts** |
| Needs a repository? | no | no |

Treating these as one thing is the first mistake available. A is arithmetic that occasionally
escalates; B is reasoning that fans out. Neither wants a worktree.

## The generic mechanism already exists: ADR-0019

`Work::Task { service, args }` beside `Work::Agent { .. }`, replacing today's agent-shaped
`RunSpec`. Accepted 2026-08-24, design ahead of code, and named in `docs/ROADMAP.md` as a phase 8
item — "the largest single edit since capabilities became instances".

It is workload A almost verbatim, and it was derived from a different scenario entirely (a phone
that can watch an API but cannot host an agent), which is the strongest thing that can be said for
a design. Point by point:

- **The work is a program the owner nominated, addressed by service** (`[[tasks]]`, `--task
  watch-shops`), never a command from the submitter. So the outbound network access to 40 shops is
  the *owner's* grant, and the submitter never names a URL. ADR-0019 §1 makes this the load-bearing
  choice rather than a convenience, and it is the reason polling 40 third-party sites does not
  need an allowlist story of its own.
- **A task has no prompt and no workspace.** No repository, no worktree, no checkout to reclaim.
- **A task's log is its output** — stdout and exit status in the run's event log, which
  `offload logs` already serves from the holder. No new plane.
- **Recurring work is a schedule firing short `Idempotent` runs**, not a long-lived run that
  sleeps. So a device that is asleep simply does not bid, and the fleet's existing machinery does
  the work. §3's derived `RunId` — a UUIDv7 whose clock bytes are the tick and whose remainder
  digests the schedule — is what stops two transient owners from firing one tick twice.
- **A missed tick is not made up.** Correct for both workloads: a watcher's value is the current
  state.

Everything the fleet already does then applies unchanged — bidding, leases, epochs, arbitration,
migration, `explain`, the audit log, the delivery plane. That is ADR-0019's own stated payoff and
it is not a small one.

**So the answer to "is there something generic worth implementing" is largely: yes, and it is
already written down and agreed.** This use case is its second independent motivating scenario.
What follows is the part ADR-0019 does not reach.

## What ADR-0019 does not reach

### 1. It frees tasks from the repository. It does not free agents.

`Work::Agent` in §2 still carries `workspace`. So after the whole edit lands, workload B — an
agent that reads changelogs and judges them against 40 shops' configurations — still has to name a
repository it does not want, does not use, and will leave an empty worktree behind on.

This is small next to the rest of the ADR and it is not mentioned in it, because the scenario that
produced ADR-0019 had no agent in it at all. The fix is a field becoming optional inside a variant
that is being introduced anyway; the design question underneath is whether a repo-less agent run
is coherent, and it appears to be: the agent gets a scratch directory, `Resumable` still works
because the transcript is what moves, and `WorkspaceSpec`'s absence is exactly the "empty rather
than zero-that-means-unknown" shape ADR-0019 §2 already argues for on `RunProgress`.

Worth raising while the `RunSpec` split is still unbuilt, because it is nearly free then and a
second wire change later.

### 2. The baseline: a cold start is the answer, and it validates `Idempotent`

Stated by the operator and it dissolves what looked like the blocker: **no state needs to move. A
poller should be able to start cold on a new node.** So there is no carried baseline, no new blob,
no new owner, and no ADR-0005 checklist to work through. `Restartability::Idempotent` — "safe to
re-run from the original prompt on a clean workspace, no state moves" — is simply correct here.

What makes a cold start free is not obvious, though, and it is the part worth writing down,
because it is a property of the *predicate* rather than of the fleet:

- A **state-based** predicate asks *is it wrong now?* — orders stuck, endpoint 500, disk full,
  installed version behind available version. A cold start answers it correctly on the first poll.
  Nothing is lost by moving nodes, ever.
- An **edge-based** predicate asks *did it change?* — this payload differs from the last one. A cold
  start loses exactly one edge: whatever changed during the gap. The new node has no last one.

Both workloads here can be phrased either way, and both should be phrased the first way. "Is there
an update available" is `installed != available`, which is state; "a new version appeared" is an
edge, and it is the same question phrased so that a cold start breaks it.

**This is worth an amendment to ADR-0019 rather than a change to it.** The ADR's choice of
`Idempotent` is right. Its motivating example is phrased in the one way that defeats that choice —
*watch an API and report when a response changes* — and a reader implementing from it will build
the edge-based version and discover the gap on the first failover. The rule that belongs beside
`Restartability::Idempotent` is: **a task that must survive a cold start needs a state-based
predicate, and phrasing a check as an edge is what makes migration cost something.**

Where genuinely edge-based work exists, the state lives in the subject or outside the fleet — a
health endpoint that reports its own revision, a database the task can reach. Neither needs
anything from Offload, and the second is the only one that undercuts "a phone can host something".

### 3. The fence protects a workspace; a task's entire purpose is effects outside one

Everything that makes double execution survivable here — epochs, leases, worktrees, *prefer a run
stalled over a run duplicated* — protects **a repository**. A task issues a refund, restarts a
queue worker, upgrades a plugin on a live shop. None of that is fenced by anything.

This gets sharper under ADR-0019, not softer, for two reasons the ADR does not connect:

- A task is `Idempotent` **by design**, and idempotent means *re-runnable*. The re-run is the
  recovery story. So the fleet will deliberately run the program again after an ambiguous
  departure, and whether that is safe is a property of the owner's program that nothing checks
  and nothing can check.
- §3's derived `RunId` makes two firings of one tick converge to one *record*. The ADR is careful
  here — "a duplicate record merges, and a duplicate agent does not" — but a duplicate **program**
  is not addressed, and two nodes that both started the program before the merge settled have both
  already had their effects.

Generic: **a run whose effects leave its workspace has an at-most-once problem the workspace model
does not address.** True of any run that sends mail, posts to an API, or deploys. Today's only
honest answers are outside the product — make the operation idempotent, or hold it behind `--ask`
so a person is the fence — and both deserve to be stated somewhere rather than discovered.

### 4. A watcher's silence is unreadable, and that one is a defect today

Independent of everything above; it is already broken. `TriggerState`
(`offload-node/src/trigger.rs:91`) carries `watching`, `events`, `restarts`, `last_error` and **no
timestamp**, with counters that reset when the daemon does. `rules.last_fired_ms`
(`offload-store/src/schema.rs:276`) is stamped only when a rule *fires*.

So a poller whose curl started returning 403 at 02:00 reads as: process up, `restarts` 0,
`last_error` `None`, `events` frozen — indistinguishable, in every report the product has, from
forty healthy shops. That is *unknown is not none, and unknown is not good news* broken from the
inside, and the inbound twin of the delivery plane's own lesson that a route which stopped being
retried is not a route.

A last-seen timestamp answers most of it for nearly nothing. A declared quiet-period expectation
answers the rest, and is **not** the interval ADR-0020 §1 refused: that refusal was about the
daemon acquiring an opinion on *missed ticks*, and there is no tick here and nothing is made up
afterwards.

### 5. The occurrence gate is keyed on the rule; the unit meant is the subject

`in_flight` asks `rule_run_in_flight(rule.id)` (`offload-node/src/trigger.rs:297`). Right for a
mailbox or a build; wrong for a watcher over forty shops, where shop B's outage is dropped because
shop A is still being triaged.

Under ADR-0019 this becomes less pressing for workload A — a schedule fires per tick, and the
program itself decides how to treat forty subjects — but it does not disappear, because the
escalation half of A is still an agent run fired by a rule. Two obstacles, both real: naming a
subject means parsing the line, and "one line is one event, nothing else is a protocol" is
load-bearing; and a per-subject gate is unbounded concurrency, which is what the gate exists to
prevent.

There is a serious argument for doing nothing: when forty shops fail at once it is one host or one
plugin, and one run that sees all forty beats forty racing agents. That alternative is a shell
script to measure, and it should be measured before any mechanism is proposed.

## The cooperation half — settled

"Multiple devices cooperate on that work" was ambiguous and has been answered: **the fleet takes
the pieces.** Work is submitted, devices bid for it, it migrates when a device leaves, a
credential on one device is reached by proxying from a run on another, and a question reaches
whichever phone is being held. Each check is its own independent unit.

That reading is largely **built**, and it is more than placement:

- Nodes *bid*; they are not assigned to. A device decides for itself whether it will take a piece.
- A run **migrates** when its holder leaves, mid-conversation, transcript and all.
- A **resource** held by one device is reached by *proxying the call*, never by moving the
  credential. The billing system's token can live on exactly one machine while a run anywhere in
  the fleet uses it — two devices cooperating inside a single work item, today.
- A **sink** can be on a third device: the run is on the desktop, the question reaches the phone,
  the answer comes back from wherever it was answered.
- A **trigger** is on whichever device can see the thing being watched, and the work it fires runs
  somewhere else entirely.

**One job split across devices and joined** — 40 shops across 4 devices, one report — is explicitly
*not* what is wanted, so the phase 7 "run DAGs" item stays where it is and nothing here needs it.

## Robustness: a preference, not a pin

The requirement, stated late and decisive: the always-on box is the normal home, but the other
nodes should know about the work and be able to take it over while that box is down — and *when* it
runs matters much less than *that* it runs.

**That single sentence chooses between the two mechanisms**, and it is the one difference between
them that is neither cosmetic nor a matter of size.

- A **rule** (ADR-0020 §2) is node-local and never gossiped, and does not fail over. The ADR states
  this rather than leaving it to be discovered: *a rule on a device that is off does not fire, and
  nothing else picks it up. That is correct and not a gap.* The trigger is that machine's owner's
  program, so migrating the rule would mean migrating a program the fleet never saw.
- A **schedule** (ADR-0019 §3) is a gossiped fact with an owner, and the owner rule is already
  exactly the requirement: **the node the schedule was created on while it is available, else the
  lowest-id available node** — arbitration's own successor rule, no election. §3's derived `RunId`
  exists *because* that rule can transiently name two owners, so two nodes firing one tick converge
  to one record.

So the robustness requirement is not a new feature request. It is already the accepted design, and
it is the reason to want ADR-0019 rather than to keep stacking things on ADR-0020.

### The hold-down already exists, is adaptive, and its defaults point the wrong way

"Hand over when the node has been gone an hour, not when it blinks" needs **no new communication
and no new mechanism.** Nodes already probe every second, and every input such a threshold wants is
already observed and already durable:

- `NodeView.last_heard`, `absent_since`, `absences`, and `typical_absence` (`view.rs:~76-95`),
  persisted in `node_observations` so they survive a restart — "a restart is precisely when a peer
  has just been absent".
- `policy::grace_for` (`offload-core/src/policy.rs:560`) — the ADR-0007 drop-off hold-down, already
  wired to all of it. It waits `typical_absence × return_factor` rather than a fixed number, so it
  *learns* that this laptop is usually away twenty minutes and that VPS never is; it adds
  `backoff_per_attempt` per previous move so a run cannot ping-pong; and it clamps the result.
  `a_brief_absence_does_not_move_the_run` and `a_long_absence_moves_the_run` are both tests
  (`policy.rs:1212`, `:1232`).

So the requirement is a **tuning and a plumbing** question, not a design one. Three specifics:

**1. `arbiter_for` has no grace at all.** It moves the moment `NodeStatus::is_gone()` is true
(`view.rs:677`), and `is_gone()` is `Dead | Draining | Departed` (`view.rs:50`), reached after
`suspect_timeout` — five seconds by default (`detector.rs:47`). Run *reassignment* takes the
patient graced path; run *arbitration* takes the bare one. Under ADR-0019 §3 a schedule's ownership
follows the arbiter rule, so schedules would inherit the five-second path and flap exactly as the
operator says they must not. That is the actual gap, and it is narrow.

**2. `max_grace` is ten minutes**, so even the graced path cannot express an hour today
(`policy.rs` defaults: `min_grace` 15s, `default_grace` 45s, `max_grace` 10min).

**3. The `Idempotent` multiplier is backwards for a task, and this is the real finding.**

```
idempotent_factor_percent: 50   // "Cheap to restart from scratch, so be less patient."
```

That reasoning is right for an agent run on a clean workspace, where cheap-to-re-run and
safe-to-re-run are the same property. **For a task they come apart.** A program that restarts a
queue worker, issues a refund or upgrades a plugin is *cheap* to re-run and not *safe* to re-run —
and the policy reads the cheapness and shortens the wait, making duplicate execution more likely
for precisely the work where duplication does damage. Compounded: an `Idempotent` run on a `Dead`
holder gets `typical × 150% × 40% × 50%` — thirty percent of a typical absence, floored at fifteen
seconds.

This is §3 arriving as a tuning constant rather than as an architectural gap, which makes it much
more likely to be shipped and never noticed. **`Restartability::Idempotent` is one word for two
facts** — "re-running is correct" and "re-running is cheap" — and this project has now split that
kind of conflation four times (`LogKind::Failed`, `Halt`, position-versus-spend, and ADR-0019 §2's
own refusal of an `Option<Task>` beside the agent fields). A task whose effects leave the machine
wants the *opposite* multiplier from an agent run that can be replayed on a fresh worktree.

Two refinements, both free:

- **The threshold applies to silence, not to a goodbye.** `is_gone()` folds three statuses
  together, but a `Draining` or `Departed` node *said* it was leaving, and the project already
  leans on that distinction ("a node knows when it is leaving, and `Draining` reached us because it
  said so"). A deliberate departure should hand schedules over at once; only the inferred `Dead`
  should wait out the threshold. `dead_factor_percent: 40` currently does the reverse — it shrinks
  the wait when the evidence is an inference.
- **A per-schedule override belongs with `deadline` and `demand`**, as a statement about how much
  interruption the work tolerates — not among the owner gates ADR-0019 §4 is careful to keep out of
  a submitter's reach. Two nodes observing the deadline slightly differently both take over and
  converge on one record, which is what §3's derived `RunId` was built for.

**"That it runs, not when" resolves cleanly too, and does not reopen §3.** ADR-0019 §3 refuses
*catch-up*: a phone asleep for six hours does not wake to seventy-two occurrences. That refusal is
about ticks that never happened. It says nothing about a tick that *did* happen and has not been
placed yet — which is an ordinary `Pending` run, live in the fleet-wide view, biddable by any node
the moment one frees capacity. `--queue` is therefore the right setting for this work and
`--deadline` is the wrong one: due as soon as somebody can take it, never refused for being
momentarily unplaceable (ADR-0046). The distinction worth holding on to is that **a deferred
occurrence is not a made-up one**.

### Coming back: three cases with three different right answers

"A node goes away, others pick up its work, and when it returns it should reclaim it" is three
questions wearing one sentence, and two of them are already answered — in opposite directions.

**1. An occurrence that was orphaned, with no decision taken yet — reclaim, and it is built.**
`Run::reclaim` (`offload-core/src/run.rs:790`): *the original holder came back before we gave up on
it. No migration, no epoch bump, no lost work — the cheapest possible outcome of a device dropping
off.* It requires the same node and the same epoch, is not counted as a new attempt, and
`a_different_node_cannot_reclaim` (`run.rs:1349`) is a test. This is the vocabulary's **Orphaned**:
*holder out of contact, no decision made yet.*

**2. An occurrence already reassigned — reclaim must be refused, and it is.** A reassignment bumps
the epoch, `reclaim` demands the epoch it left at, and so a returning holder is fenced out. This
must stay exactly as it is: it is *double execution is the failure mode that matters*, and the
returning node is the one that cannot tell it has been superseded. For a thirty-second poll there
is nothing worth reclaiming anyway; for workload B's long agent run this is the whole safety story.

**3. The schedule itself — and this is what was actually meant. It is free, and it already works.**
`arbiter_for` is a **pure function of the current view**, not a stored assignment: home while home
is not gone, else the lowest-id available node (`view.rs:675`). So when the box returns and is
`Alive` again, ownership is simply back — no handshake, no message, no stored claim to release, and
nothing to migrate because a schedule holds no state and the next occurrence starts cold. What that gets right is that nothing has to be *moved*. What it gets
wrong is treating the handback as instantaneous: between the box returning and the successor
noticing, both compute themselves the owner, so a return is a **negotiation** and not a snap — see
the affinity section below, where it turns out to be the same mechanism as expressing the
preference in the first place.

Under ADR-0020 none of this is available: a rule is node-local and never gossiped, so nothing picks
it up while the node is away and there is nothing to hand back. That is the third independent
reason this workload wants schedules rather than rules.

#### The one hazard, and §3's derived id already answers it

Handover is deliberately slow; return is instant. That asymmetry is right — waiting protects
against a blip, returning costs nothing — but it leaves one window: **a returning owner can fire a
tick the successor has already fired.**

ADR-0019 §3 uses the derived `RunId` to *merge* — two firings of one tick become one record. The
same id supports something the ADR does not spell out, and it is the more valuable use: because the
id is computable **offline by every node from the schedule and the tick alone**, an owner about to
fire can first ask whether that record already exists, and decline. Merge happens after the fact
and protects the *record*; the check happens before and protects the *execution*.

It is not airtight — there is a window between the check and the start, with no lock across nodes —
but it turns "a duplicate program run every time ownership moves" into "a duplicate only inside one
gossip round", which for work with external effects is the difference that matters. It is also the
only partial answer §3 has, so it is worth stating in the ADR rather than leaving to be rediscovered.

The open half is whether **return** should be hysteretic too. A box flapping up and down every
thirty seconds oscillates ownership, and every oscillation reopens that window. The same absence
history that decides how long to wait before handing over (`typical_absence`, `absences`) already
knows the node is flappy, so the symmetric rule — *be stably back before taking it back* — needs no
new observation either.

## Affinity: "run it on the VPS, but not *only* on the VPS"

The requirement, and it is the one that unifies the rest: *I want this poller on my VPS because
that box is always up. But there is a maintenance window, and I still want it to run somewhere
meanwhile — and when the VPS is back it should get the work again.*

### There is no soft constraint, and that is the whole gap

The two axes that exist are both wrong for this, in opposite directions:

- **`Constraint` is hard.** A boolean tree over capabilities, consumed as eligibility, paired with
  `explain`. `HasTag(String)` and `Label(String, String)` (`constraint.rs:44-45`) already let an
  owner label the VPS, so *naming* the box is not the problem. The problem is that
  `Constraint::HasTag("always-on")` means **only** there: during the maintenance window nothing
  matches, and the work does not run at all — the opposite of what was asked for.
- **`BidWeights` is not the submitter's.** Every field is a property of the node or of its
  relationship to the run — `stability`, `workspace_warm`, `checkpoint_local`, `load_penalty`,
  `migration_penalty`. There is nothing a submitter can put on the scale, and roadmap question 7
  notes `BidWeights::default()` is the only constructor, so nothing can configure them at all.

So the system has **must** and it does not have **should**. That is the gap, it is one sentence,
and it is not specific to this workload: "prefer the desktop for builds, but anywhere beats
nowhere" is the same request.

**`Restartability::Pinned` is the wrong tool and is the first thing anyone will reach for.** "Cannot
move. Fails if its holder goes away" is a hard pin — it turns the maintenance window into an outage,
which is precisely the thing being designed against.

### The shape: reuse `Constraint` as the preference language

A second `Constraint` beside the first, consumed by scoring rather than by eligibility. Same tree,
same `matches`, same `explain`, different consumer — a preference is a requirement that does not
disqualify. Nothing new to learn, nothing new to parse, and `explain` gains a sentence it can
already almost write: *placed here; your preference was not met, and here is who did meet it and
why they did not win.*

That keeps the house rule intact. **Nodes still bid; they are not assigned to.** A preference tilts
a score, and a node that will not take work still will not take it.

### Affinity and reclaim are the same mechanism

This is why the two halves of the requirement arrived in one message.

- A **preference** is a standing statement about where the work belongs.
- **Reclaim** is that statement being re-evaluated after the preferred node comes back.

And the negotiation is already built: it is the bid round, and the incumbent already has an
advantage in it — `migration_penalty`, *"charged when the run currently has a healthy holder
elsewhere: moving work that is fine where it is should need a clearly better offer."* That is
exactly the hysteresis a handback needs, already present and already justified in the right words.

So "the VPS reclaims its poller" is not a new verb. It is: the VPS returns, bids, and its affinity
is worth more than the incumbent's migration penalty. A preference too weak to clear that penalty
means the work stays where it is until the next natural boundary — which for a schedule is the next
tick, and is free.

**For a schedule this re-evaluation is continuously correct**, and that is worth separating from
roadmap question 9 ("should a healthy run move to a better node that just freed up?", leaning *no*).
The reason the answer differs is not a change of mind: a **run** holds a transcript, a worktree and
a turn's worth of progress, so moving it costs a checkpoint and at most a turn. A **schedule** holds
nothing at all, so moving it costs nothing, and continuous re-evaluation is not thrash — it is just
the answer. The two cases differ by exactly the state they carry.

### The maintenance window is a drain, and that is a goodbye

Worth saying because it removes the hard part of the scenario. A planned maintenance window is not
a node going silent; it is `offload drain` — stop accepting, hand off, say so. The fleet learns
`Draining` rather than inferring `Dead`, which is the distinction the hold-down section leans on
from the other side: **wait out silence, act on a goodbye.** So the maintenance case needs none of
the absence heuristics; it needs only that the affinity survives the departure and re-asserts on
return, which is what a standing preference on the schedule is.

### "It depends on the type of work" — the part that stays open

Whether a handback may happen *at all* mid-flight is §3 again, and it is the one thing here that a
preference cannot answer. A read-only poller tolerates a sloppy handover: worst case, two polls.
A task that restarts a queue worker or issues a refund does not. That is the task declaring its own
tolerance, which is what item 7's ADR is for — and it is the reason "reclaim is a negotiation"
is the right instinct rather than a detail: **for some work the correct handback is to wait for the
current occurrence to finish, and for some it is to never interrupt at all.**

## The poller with memory, and the filter

The refinement that arrived last: *the poller could have memory and only hand work to an agent if
the payload changes — or if it changes in a particular way, say a named field in a JSON body.*

**Stage one of that is built, today, and it is not a task — it is a trigger.** A long-lived program
that polls on its own clock, holds the last payload in memory, applies whatever predicate its
author likes, and prints a line only when the predicate fires. That line fires a rule; the rule
submits an agent run. `curl … | jq -e '.orders.stuck > 0'` in a `while` loop is the entire
mechanism, and `jq` is the filter language — mature, with an answer already for missing fields,
deep equality and non-JSON input.

So the question is not *can the fleet do this*. It is **what the poller costs by being a trigger**,
and there is exactly one cost, stated in ADR-0020 §2 rather than discovered: *a rule on a device
that is off does not fire, and nothing else picks it up.* The watcher is one machine's program. It
does not migrate, because migrating it would mean migrating a program the node never gossiped.

That cost is what the robustness requirement above refuses to pay, so the fork it opened is
already closed: **the poller has to be fleet work, which means ADR-0019 and not ADR-0020.** What
survives from the trigger version is the good part — the program owns its cadence, its memory and
its predicate, and the orchestrator learns none of the three.

### The filter belongs in the program, and both ADRs say so

Putting the change rule in Offload — `[[tasks]]` with a `filter = "$.orders.stuck"` — is a
relitigation of a decision taken twice, in the same words both times:

> ADR-0019, rejecting `Work::HttpPoll`: *it puts an HTTP client, **a change-detection rule** and a
> retry policy inside an orchestrator, which is "never reimplement an agent" arriving in a
> different costume.*

> ADR-0020 §1: *there is no interval field, no cron parser, no HTTP client and **no
> change-detection rule** anywhere in the orchestrator.*

The reason holds and it is ADR-0020 §4's own argument about templating, one level up: *a little
template language would be a quoting bug with a syntax*, and a little filter language is a
change-detection engine with a syntax. A field path needs comparison semantics (deep equal?
numeric tolerance? array order?), a missing-field answer, a not-JSON answer, and a way to say
"changed by more than". Every one of those is a decision Offload would own and get wrong for
somebody.

And the decisive one: **the filter needs the previous payload, so a filter in the orchestrator
drags the state problem into the orchestrator with it.** §2 and the filter are the same question.

### What is generic here: a task's output and a trigger's events are the same pipe

The one new mechanism this use case suggests, and it adds no concept at all.

- A **trigger** is a *long-lived* program whose stdout becomes **events**. It cannot be scheduled;
  an exit is a restart with backoff, so a poll-once-and-exit program becomes a hot loop.
- A **task** (ADR-0019) is a *short* program, fired by a schedule, whose stdout becomes a **log**.

They are the same pipe read from opposite ends. Let a scheduled task's stdout be an event stream —
the composition of two mechanisms that already exist rather than a third — and the poller becomes
fleet work: bid for, placed, migratable, explained by `explain`, visible in `ps`, with a missed
tick not made up. The program still decides what to print, so **no change-detection rule enters the
orchestrator**; what changes is only where the program runs and who decides when.

That leaves the baseline as the single remaining blocker, which is a good place for a design to
narrow to: everything in this section reduces to §2, *where does last time's payload live*.

## What would be worth doing, in order

The shape that settled: **ADR-0019 is the mechanism; the robustness requirement is why; and almost
nothing new has to be built.** No carried state, no fan-out, no change-detection engine, no new
communication. What is left is one defect, one amendment, and one constant that is right for agents
and wrong for tasks.

1. **Phrase every predicate state-based** (§2). Free, outside the code, and it is what makes a cold
   start cost nothing.
2. **Fix the `Idempotent` hold-down for work with external effects.** The highest-value item here:
   `idempotent_factor_percent: 50` shortens the wait for exactly the runs where a duplicate
   execution does damage, and it will ship unnoticed because the reasoning behind it is correct for
   the case it was written for. Splitting "re-running is correct" from "re-running is cheap" is the
   change.
3. **Give `arbiter_for` a grace, or give schedule ownership its own.** Five seconds is right for
   arbitration and wrong for owning a schedule; `max_grace` at ten minutes cannot express an hour.
4. **A soft constraint — `prefer` beside `constraint`.** *(Accepted as ADR-0063 in session eighty-eight, from the owner's own placement scenarios — phase 10.)* The one genuinely missing primitive
   this use case found, and it is small: a second `Constraint` consumed by scoring instead of
   eligibility. It is also what makes affinity, reclaim and the maintenance window one mechanism
   rather than three, and roadmap question 7 should be answered in the same breath, since a
   submitter-supplied term on the scale is the first thing that ever puts a number there.
5. **Check the derived `RunId` before firing, not only when merging.** Small, uses machinery
   ADR-0019 §3 already specifies, and it is the only partial defence §3's duplicate-execution
   problem has. Belongs in the ADR as a numbered decision rather than as an implementation detail.
6. **Make a watcher's silence readable** (§4). Small, generic, already broken, and it matters more
   under this design because a correct poller is a silent one.
7. **Amend ADR-0019 before it is built** with §1 (optional workspace on `Work::Agent`) and §2's
   state-based-predicate rule. Nearly free while the `RunSpec` split is unwritten; a second wire
   change after.
8. **Decide §3 as an ADR** — what a task may declare about its own re-runnability. Items 2 and 3
   are the tuning half of this; the ADR is the half that says what a program is allowed to promise,
   and it is also what decides whether a handback may interrupt an occurrence or must wait it out.
9. **Measure §5's do-nothing alternative** before proposing a per-subject gate.

## Open questions

- **Can the subject report its own revision?** Answering yes removes §2, and §2 is the blocker.
- Is a task's exit a checkpoint boundary? ADR-0019 §2 says a task has no boundary and therefore no
  capture; the opposite reading is that it has *only* boundaries and the capture is trivially safe.
  Whichever is right, the ADR should say which and why.
- If a baseline travels, who owns it and what arbitrates two of them? The derived `RunId` merges
  two firings into one record but not into one execution, so both may have written one.
- Is `prefer` a `Constraint` reused, or does a preference want its own smaller language? Reuse is
  the cheaper answer and probably right, but `MinStability` as a *preference* reads oddly.
- Should *returning* be hysteretic as well as handing over? The absence history needed to decide
  is already collected.
- Is a repo-less agent run coherent enough to be a variant, or does an agent always want *a*
  directory and the honest answer is a scratch workspace with a name that says so?
- If a scheduled task's stdout can be an event stream, does a trigger remain a separate concept or
  become the degenerate case — a task whose schedule is "always running"?
- Does `Work::Task` want a *fleet* of subjects as a first-class notion, or is "the program handles
  forty shops however it likes" the right amount of orchestration? The second is much more in this
  project's voice, and may simply be right.
- Does any of this belong in `docs/ROADMAP.md` yet? §4 probably does now, as a defect. The rest are
  amendments to an accepted ADR, which is a different act.
