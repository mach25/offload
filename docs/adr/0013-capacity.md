# ADR-0013: Capacity is a device budget with reservations, not a count of runs

**Status:** accepted · 2026-07-26

## Context

Admission control today is a number. `WorkPolicy::max_concurrent_runs` is compared against
how many runs a node is hosting, and `Refusal::AtCapacity { running, max }` is what a node
says when the number is reached. That is the right shape for a fleet where every run costs
about the same, and agent runs do not.

The scenario this ADR exists for: a node accepts a run that turns out to be heavy — a long
build, a large repository, an agent that spends forty minutes executing its own test suite.
For the duration, that node should not take on more work, and when a second run is offered
it should either decline it clearly or say *not yet*. Counting runs cannot express this. One
heavy run and one trivial one are both "1", so a laptop mid-build looks exactly as available
as a laptop doing nothing, wins the bid on its hardware score, and then serves both runs
badly.

Three complications make it more than a bigger number:

- **Cost is not knowable in advance.** An agent run's demand is whatever the model decides
  to do. A run submitted as "fix a typo" can start a full rebuild in its third turn. Any
  scheme resting entirely on a declared weight will be wrong regularly.
- **Preemption is off the table.** ADR-0004 says mid-turn is not a safe checkpoint, so a
  busy node cannot make room by evicting what it is running. Its only honest moves are to
  refuse, to defer, or to finish.
- **The device is not the node.** ADR-0012 makes multi-fleet membership a matter of running
  one `offloadd` per fleet, and neither instance knows what the other has accepted. Two
  fleets each permitted two runs can leave one laptop hosting four. Capacity is a property of
  the machine, and nothing currently owns it at that level.

## Decision

**Capacity is a budget, and runs consume shares of it.** `WorkPolicy` gains a budget
alongside its count — the count survives as a hard ceiling, because "never more than two
agents on my laptop regardless" is a thing an owner reasonably wants to say.

A run declares its expected demand, coarsely. Fine-grained resource declarations are a
fiction when the workload is a language model with a shell:

```rust
/// What hosting this run is expected to cost the device.
pub enum Demand {
    /// Seconds of work, negligible CPU, no build. A scheduled check, a review run.
    Light,
    /// The default for an agent run: sustained model latency, some tool execution.
    Normal,
    /// Known up front to be expensive — a full build, a large clone, a long suite.
    Heavy,
}
```

**When three levels stop being enough, the answer is dimensions, not more levels.** Compiling
a large project and encoding video are both "heavy" and are not alike: one saturates CPU and
disk, the other one GPU or every core for an hour. Two runs that saturate *different*
dimensions coexist happily; two that hit the same one do not — and no scalar expresses that at
any resolution, so a finer scale would be wrong with more decimal places. The growth path is

```rust
struct Demand { cpu: Level, memory: Level, io: Level, gpu: Level }
```

still coarse per axis, but composing correctly: an agent run and a video encode can share a
machine, two encodes cannot. Not now — for the workloads Offload orchestrates today, demand
barely matters at all, because agent runs are model-latency bound. It becomes worth building
the first time genuinely build-shaped or encode-shaped work arrives.

**Hardware requirements are constraints, not demand.** "Needs NVENC", "needs 16 cores" belong
in `Constraint`, which already expresses them. The distinction is not stylistic: a wrong
constraint means the run *cannot work*, while a wrong demand means it merely runs somewhere
suboptimal. Keeping them apart is what stops a scheduling estimate from quietly becoming a
correctness requirement.

**Declared demand is a hint; observed pressure is the truth.** Admission takes the greater
of what the run claimed and what the node is actually experiencing — `LocalFacts` already
carries `cpu_load_percent`, and this is what it is for. A node whose declared budget says it
is half free but whose load average says otherwise refuses, and says which:

```rust
Refusal::UnderPressure { load_percent: u8, budget_free: u8 }
```

This is the same rule as the probe's: do not claim what cannot be verified. A node that
accepts work on the strength of an optimistic declaration and then serves it badly is worse
than one that declines.

**A busy node defers rather than refusing, when it can say when.** `NoBid` gains a variant
that is not a "no":

```rust
NoBid::Busy { retry_after: Millis, reason: Refusal }
```

The distinction matters to the arbiter and to the human. `Refused(BatteryTooLow)` means look
elsewhere; `Busy { retry_after }` means this node is the right one shortly. `offload explain`
prints both, which is the whole point of keeping refusals structured.

The estimate is a hint and never a promise — the run stays `Pending` regardless, and any
node may take it in the meantime. An agent run's remaining time is not predictable, so
`retry_after` is derived from what the node knows (turns elapsed, typical turn duration) and
is explicitly allowed to be wrong.

**Capacity changes are events that trigger re-bidding.** This is what makes "later" real
rather than aspirational: when a run finishes and frees budget, the node re-evaluates the
pending runs it declined and bids on those it can now serve. Without it, deferral is just a
politer refusal and the run waits for the next thing to happen to it.

### Deadline, precedence, attendance — three things, not one level

`Busy { retry_after: 500s }` is information nobody can act on until the run says whether 500
seconds is acceptable. The tempting answer is a single urgency level, and it is wrong: an
earlier draft of this ADR had `Interactive | Normal | Batch`, which quietly conflates *"this
can wait"* with *"this should wait"*.

**"Finish by tomorrow morning" is not urgent and is not a reason to sit idle.** If nothing
else wants the machine, start it now. Yield only when something more urgent turns up. One
ordinal cannot say that, because it answers "how soon" and "who goes first" with the same
number. So the concept splits into three, two of which already exist:

```rust
/// When this needs to be done. `None` means no deadline, not "no hurry".
pub deadline: Option<Millis>,
/// Who yields to whom when capacity is contended. Already in RunSpec.
pub priority: i32,
```

- **`deadline` — how long a deferral may be.** This is what makes `Busy { retry_after }`
  judgeable: a deferral is a bid if the run still lands comfortably before its deadline, and
  a decline if it does not. Absolute time, so it is compared against an injected `now` and
  the core stays clock-free. "By 08:00 tomorrow" is expressible exactly; `None` means
  nothing is waiting on it.
- **`priority` — who yields.** Already present, described as advisory, and it is the field
  that carries "let more urgent things go first". Crucially, low priority does **not** mean
  delayed: a low-priority run on an idle fleet starts immediately. It only loses when
  something else wants the same capacity at the same moment.
- **Attendance — whether anyone is actually watching**, which is *observed rather than
  declared*: a run is attended while some client is streaming it (`offload run --follow`,
  `offload logs -f`), and unattended otherwise. Nobody has to predict it at submit time, it
  is never stale, and it changes correctly when you walk away — which is exactly when a run
  should start behaving more autonomously.

The three answer three different questions, and every attempt to serve them with one level
loses one: *how long can this wait* (deadline), *who goes first when two things want the same
machine* (priority), *is there a human to escalate to* (attendance).

A worked version of the case that prompted this: a repo-wide refactor due tomorrow morning
gets `deadline = 08:00`, a low `priority`, and is unattended once you close the terminal. It
starts immediately on the idle desktop; it yields that desktop to a run you submit at noon
and waits, because a 40-minute deferral still lands well before 08:00; and when the desktop
drops off at 02:00 it auto-resumes elsewhere rather than waiting for a human who is asleep.

**Urgency is derived, and it rises by itself.** This is the strongest argument for a deadline
over a stored level: urgency is not a field at all but a pure function of `(deadline, now)`,
so a run gets more urgent as tomorrow morning approaches without anybody editing anything and
without a single message being exchanged. Every node computes the same value at the same
instant from state it already has, so there is nothing to gossip, nothing to arbitrate, and
no revision counter — the properties a stored urgency level would have needed all of.

The deferral comparison inherits that for free, and correctly changes its answer over time:
a `Busy { retry_after: 500s }` that is a perfectly good bid at 14:00 for an 08:00 deadline is
a decline at 07:55. Same run, same node, same offer — different amount of slack.

Slack is `deadline - now`, deliberately not `deadline - now - estimated_remaining`, because
the remaining duration of an agent run is exactly the thing nobody can estimate. Refinement
by observed turn rate is possible later; it is not load-bearing and must not become so.

**An unspecified deadline means the moment it was submitted.** Not "no hurry" and not "no
deadline" — *as soon as you can*, which is what somebody typing `offload run` with no further
qualification actually means. The field stays `Option<Millis>` so the display can say `asap`
rather than a bogus timestamp in the past, but the arithmetic treats `None` as
`run.created_at`, which already exists.

This is worth more than the convenience. It makes slack equal to negative age, decreasing
monotonically for as long as the run waits — **so aging is free, and starvation is impossible
without a starvation mechanism.** A queue of runs submitted with no deadline is ordered by how
long each has been waiting, because that is exactly what their urgency measures. No
`pending_since` special case, no aging function, no mutable state: still `f(deadline, now)`,
still identical on every node.

Two rules follow from slack being routinely negative:

- **Overdue runs get less picky, not more.** The deferral test — does this `retry_after` fit
  in my slack — has no sensible answer once slack is negative, and the tempting reading
  ("refuse everything, I am already late") is exactly wrong: being picky is how a late run
  gets later. Below zero, a run takes the earliest available placement rather than holding out
  for a better one. Above zero, it can afford to wait for a better node, which is the case a
  deadline exists to express.
- **Priority is a head start, not a veto.** Urgency grows without bound as a run waits, so any
  run eventually outranks any priority. That is the anti-starvation property, and it is why
  priority can stay a simple advisory integer rather than needing fairness machinery of its
  own.

**Ordering is rough, deliberately not guaranteed, and frequently irrelevant.** Nodes bid
independently; there is no queue and nothing to serialise. Ten runs fired off at a fleet with
four idle capable nodes do not queue at all — four start immediately, and ordering only
becomes a question once capacity is scarce. That is the normal case and the point of the
fleet: the answer to "what order will these run in" is usually "at the same time, elsewhere".

What holds when capacity *is* scarce is weaker than a queue and sufficient: nothing waits
forever, and among the runs a node could take it prefers the most overdue, with priority as the
tiebreak. A batch fired off together comes out roughly in the order it went in, and "roughly"
is the honest word — a warm workspace on one node will reorder two runs submitted a second
apart, and should.

A run with a real deadline still cannot starve past it by more than the queue ahead of it: its
slack shrinks toward zero as the hour approaches, and at zero it is competing on the same
terms as everything else. **A deadline is when a run starts competing hardest, not when it is
promised to be done.**

Manual change (`offload deadline`) therefore covers only the case time cannot: *my needs
changed*. The case of *it is getting late* is already handled by the clock.

**The deadline is a scheduling input, not a promise.** Missing it produces a notification
(ADR-0010), never a cancellation. A run that cancelled itself because a laptop closed would
be a far worse surprise than a late one, and "cancel if it cannot make the deadline" is a
per-run option that is deliberately not being invented now.

**Clock skew is tolerable because deadlines are coarse.** Nodes disagree about `now` by
seconds; deadlines are hours away. Phase 6's simulation should still include skew, and any
future deadline measured in seconds would need to be reconsidered rather than trusted.

**The deadline is mutable; everything else about a run's spec is not.**

```
offload deadline <run> 08:00        # I need this by morning after all
offload deadline <run> none         # actually, whenever
offload priority <run> +10          # and let it go ahead of the others
```

Changing your mind about when you need something is ordinary — the run that was fine overnight
becomes the one blocking a demo. So the deadline (and priority, which was always advisory)
can be edited after submission, and that comes with the obligations any mutable gossiped field
has (CLAUDE.md, ADR-0005):

- **The home node owns it**, arbitrated by a revision counter it bumps — the same node that
  accepted the submission and stands in for the operator, with the arbiter's deterministic
  successor taking over if it is away. An operator can issue the change at any node; it is
  forwarded to the owner rather than applied locally, so two devices changing it at once
  converge instead of flapping.
- **A change re-triggers bidding**, exactly as a capacity change does. Tightening a deadline
  is useless if nobody re-evaluates the deferrals that were acceptable a minute ago; this is
  the second trigger on the same mechanism, not a new one.
- **On a running run it changes nothing immediately, and the CLI says so.** There is no
  preemption and no way to make an agent think faster. It affects only later decisions — where
  the run lands if it is re-placed, how its next deferral is judged, and how patiently its
  next failure is handled. Claiming otherwise would be the most tempting lie in this design.

### All of this is inert until something breaks

On a healthy running run, none of these three fields does anything. Their real job is that
every failure path in this system ends at the same fork — **wait, or act** — and until now
each of those forks has been answered by a fixed policy that can know neither how much time is
left nor whether anybody is watching.

| Failure                          | Fixed policy today                     | Decided by      | How                                                                    |
| -------------------------------- | -------------------------------------- | --------------- | ---------------------------------------------------------------------- |
| Holder out of contact (ADR-0007) | grace from absence history             | deadline        | slack left is how long it can afford to hold down before moving         |
| Reassignment available           | migrate once the grace expires         | deadline        | whether migrating — a lost turn, a re-clone, a patch apply — fits       |
| Run failed with a checkpoint     | sits `Failed` until a human resumes it | attendance      | unattended auto-resumes; attended escalates to the person sitting there |
| Retry backoff (`attempts`)       | fixed, to stop ping-pong               | both            | slack sets the ceiling, attendance sets how eagerly to use it           |
| Nothing can be placed            | pending forever (open question #3)     | deadline        | a run that cannot make its deadline says so; one without stays pending  |
| Rate limited                     | records the event                      | deadline        | wait for the reset if it fits, surface it if it does not                |

Splitting the axes fixes something the single level got wrong here too. **Autonomy in failure
follows attendance, not deadline.** A run due in ten minutes with nobody watching should still
auto-resume, because waiting for a human who is not there is how it misses the deadline; and a
run due next week that you are actively watching should escalate to you rather than quietly
retrying, because you are right there and can fix it. An ordinal that bundled "soon" with
"watched" got both of those backwards half the time.

That is also the argument for observing attendance rather than declaring it. It is the input
that decides whether to wake somebody up, and a submit-time guess about whether you will still
be at your desk in three hours is worth very little.

This makes `deadline` an input to `grace_for` and `decide_reassignment` in ADR-0007's
hold-down policy — an added input, not a changed rule: absence history still says how flaky a
node is, and the deadline says how much of that flakiness this particular run can afford.

**And a hard boundary, because this is exactly where a scheduling hint turns into a
correctness bug:** these fields may change *when we give up*, never *what we are allowed to
do*. No deadline, however close, may weaken epoch fencing, skip a checkpoint, snapshot
mid-turn, or shorten a lease below what the failure detector needs. A deadline must never
become the flag people set to make things faster, because the first thing it would buy is the
double execution this whole design exists to prevent. When the two are in tension, the run
misses its deadline.

Escalation over time is *already solved for anything with a deadline*, because slack shrinks
without being told to. What remains open is the run that has **no** deadline: nothing makes it
more urgent, so a fleet that is permanently busy can starve it forever. That is open question
#3's other half, and the natural fix is aging `priority` on `pending_since` — which stays
compatible with the pure-core rule for the same reason derived urgency does, as long as it is
a function of `(declared, pending_since, now)` rather than mutable state. Not decided here.

**Reservations are leases, held at the device level.** A node reserves budget before
accepting a run and releases it on completion. The reservation carries an expiry for the same
reason a run lease does: a crashed instance must not hold the machine's capacity forever.

Across fleets, those reservations live in a **device-local broker** — a small file or socket
under a well-known path, shared by every `offloadd` instance on the machine, holding
`{instance, run, demand, expires_at}`. Each instance consults it before bidding and updates
it on accept and release. It is not a scheduler, a daemon, or a cross-fleet channel; it is a
ledger of what the machine has already promised.

**What crosses fleets is the number, never the work.** A second fleet learns that the device
has half its budget committed and no more than that: not which run, not which repository, not
which fleet. `Refusal::AtCapacity` and `Busy` are reported in terms of this device's budget,
which is a fact about the hardware and discloses nothing about the other fleet's contents.
Anything richer would make the broker the bridge that ADR-0012 forbids.

**No preemption, ever.** A heavy run in flight is not evicted to make room, because there is
no safe point to evict it at (ADR-0004). Making room is a deliberate act — `offload
checkpoint` or a drain — initiated by a human or by a node that is leaving, never by an
arriving bid.

## Consequences

Good:

- A busy laptop stops winning bids it should lose. This is the case the bidding design exists
  for and the one it currently gets wrong, since hardware score dominates and load barely
  registers.
- "Why is my run still pending" gains a real answer: *desktop is busy for another ~20
  minutes, laptop declined on battery, phone lacks the agent*. Deferral with a reason is
  strictly more useful than a refusal, and it is the difference between a fleet that looks
  broken and one that looks busy.
- Multi-fleet devices stop over-committing, and they do so without either fleet learning
  anything about the other.
- The budget is expressed in the owner's language — "at most half this machine" — rather than
  in run counts that mean different things on different days.

Bad, and worth being clear-eyed about:

- **Every estimate here is bad.** Declared demand is a guess, `retry_after` is a guess, and
  observed load is a lagging indicator of a workload that spikes. The design leans on
  refusing when uncertain, which means it will sometimes decline work it could have served.
- **The broker is a new shared-state failure mode** on a design that has carefully avoided
  them. A stale ledger under-commits the machine, a lost one over-commits it, and it is
  exactly the sort of file that gets left behind by a hard kill. Expiring reservations bound
  the damage without removing it.
- **Re-bidding on capacity change is a wake-up path** that has to avoid a thundering herd
  when a big run finishes on a node holding several deferrals, and avoid re-bidding storms
  when a load average oscillates around a threshold.
- **Three demand levels will be wrong.** Some runs genuinely are 20× others. The alternative
  is asking humans to estimate CPU-seconds for a language model, which is worse, but the
  coarseness will show up as a node accepting two "Normal" runs that behave like four.
- **`deadline` is the first mutable field in a run's spec**, and mutability is what the
  ownership rules in ADR-0005 exist to survive. One field with an owner and a revision counter
  is manageable; the risk is that it establishes a pattern and the *next* field arrives
  without either. `RunSpec` should stay immutable apart from this and `priority`, and a third
  mutable field deserves an ADR of its own rather than a follow-my-leader.
- **"Change the deadline" will be expected to do more than it does.** On a pending run it
  genuinely re-opens placement; on a running one it changes nothing anybody can see *while
  things go well*, because there is no preemption and no making an agent think faster. What
  it buys on a running run is different behaviour when that run breaks — invisible right up
  until it is the only thing that matters. The CLI should say that precisely rather than
  either overclaiming or shrugging.
- **A wrong deadline now has teeth.** It is not only a placement hint but a standing
  instruction about how patiently to handle failure. A deadline of "next week" on something
  you are actually waiting for buys a quiet twenty-minute hold-down on a node that is never
  coming back — and unlike a bad placement, nothing about that looks wrong while it happens.
- **Derived urgency means behaviour changes with no event to point at.** A run that was
  content to wait becomes one that migrates aggressively, and the only thing that happened is
  that time passed. That is correct and it is genuinely hard to debug from a log, so
  `offload explain` needs to report *slack* rather than only the deadline.
- **Failure handling now depends on deadline × attendance × failure mode**, which is a
  bigger test matrix than fixed policy. Phase 6's simulation should treat both as dimensions
  of the churn matrix rather than parameters to hold constant — including the case where a
  deadline passes mid-partition.

## Amendment, 2026-08-20: what building the budget changed

Four departures from the letter above, all found while implementing it. None changes the
decision; each narrows or corrects a mechanism it named.

**`Busy` is for observed pressure, not for being full.** The ADR reads as though a node at
capacity would defer with `NoBid::Busy { retry_after }`. ADR-0006's accepting-without-starting
landed in between and is the better answer for that case: a full node *commits*, takes the
grant into `Assigned`, and starts when its own queue drains, because declining leaves nobody
holding the run. So the line is **which queue the node is behind**. Being full — by the count
or by the budget — is this node's own queue: it accepted that work, it knows the depth, and it
empties by itself. Being under pressure is a condition of the machine caused by work this node
never accepted, the owner's build or another fleet's daemon; it can neither promise when that
drains nor make it drain. That is the case that defers rather than commits, and the deferral is
judged against the run's slack exactly as this ADR says — with a run that has nothing left to
spend told to look elsewhere *now*, because being picky is how a late run gets later.

**`retry_after` is a constant tied to its input, not an estimate.** The ADR allows it to be
derived from turns elapsed and typical turn duration. Nothing here has learned to measure turn
duration, and the number is derived from a *one-minute load average* — an input that cannot see
a spike shorter than a minute. So `PRESSURE_RETRY` is that window, and it is honest about being
a guess rather than precise-looking.

**A lone run always fits.** Not stated above, and the budget is wrong without it: a `Heavy` run
wants more shares than a phone's whole budget, so refusing it there turns a number the owner
picked for *concurrency* into an eligibility rule — and the run is then unplaceable across a
fleet of small devices rather than merely slow on one. The budget limits what runs *beside*
something. Hardware a run genuinely cannot do without is a `Constraint`, which fails loudly and
names itself; this ADR's own distinction, applied to its own mechanism.

**Pressure gates accepting, never starting.** A committed run has nowhere else to be, and
nothing frees the machine on its behalf, so refusing to start it until the load average
improves is a run held hostage to a number nobody controls. The honest way out of a commitment
is to give it back, which is what `review_commitment` does — and only for a deadline somebody
stated, for the same reason `grace_for` only shortens patience for a stated one: with an
unspecified deadline every held run is overdue within a second, and the rule would bounce every
commitment in the fleet from one queue to another for ever.

**Attendance is sampled at the failure, not at the decision.** The ADR says a run is attended
while some client is streaming it, which is right and is not implementable as written for the
decision it feeds: a client following a run stops following when the stream ends, and a failure
*is* the end of the stream. Asked a tick after the fact, every failed run is unattended and the
attended branch is dead code that looks correct. So the observation is taken at the instant the
run is written `Failed`, and the question it answers is *was somebody looking when it broke* —
which is the useful one anyway, because that person has the reason in their terminal already.
The escalation has, in a sense, already happened; what is left is not to retry underneath it.

**It lives in memory on the node that failed the run, and is gossiped nowhere.** A `Failed` run
is terminal to `supervise`, so recovery is the holder's decision — and the holder is the node
with the checkpoint, the warm worktree and the stream. Nothing has to travel, so nothing needs
an owner or a merge rule. A daemon that restarted between the failure and the decision reports
`None` rather than "unattended", and the answer to `None` is to leave the run for a person: the
cost of that is a run that waits, and the cost of guessing the other way is an agent restarted
behind somebody's back.

**A run interrupted by a restart is unknown, not unattended.** The shortcut is inviting —
nobody *can* be watching after a restart, since the restart disconnected them — and it hands a
crash-looping daemon every run on the machine, on every start, with no memory of having done it.
So those runs keep today's behaviour and wait for `offload resume`, with the reason said out
loud rather than left as silence.

**The deadline does not decide whether to resume, only how long to wait first.** A passed
deadline must never stop the work — this ADR is explicit that missing one produces a
notification and never a cancellation — so the backoff grows per resume, is capped by the
moment the run is due when a deadline was *stated*, and is never shortened below a floor. With
an unspecified deadline every failed run is already past due, which is the third place in this
codebase where reading that as urgency would have collapsed a wait to nothing.

**Attendance cannot inform the hold-down, and the table above is wrong about that.** The row
that wants both axes — "slack sets the ceiling on a retry, attendance sets how eagerly to use
it" — is unbuildable as written for the *holder-out-of-contact* case, and the reason is worth
keeping rather than quietly ignoring: attendance is observable only where the stream is served,
which is the holder, and the decision is taken by the arbiter about a holder that has vanished.
The one node that could answer is the one that is gone. Gossiping it would not rescue that
either: the fact's owner disappears exactly when the fact is wanted, so what the arbiter would
have is a value from before the silence, presented as current — the same confident wrong answer
`offload explain` refuses to give by replaying old bids.

Where both axes *do* meet is the case the holder is present for: a run whose agent failed. There
the recovery backoff grows per resume, is shortened by a stated deadline, and is not used at all
when somebody was watching — slack setting the ceiling and attendance deciding whether to spend
it, on the one node that can see both. The prerequisite for the other case is unrelated to
scheduling: a client following a run placed elsewhere, proxied through the fleet, which would
make attendance an arbiter-visible fact and is a feature in its own right.

**Still not built, and deliberately: the push half of "capacity changes trigger re-bidding".**
A node that frees capacity is not the arbiter of the pending runs it declined and cannot grant
itself work, so the re-bid it would trigger is somebody else's pass anyway. The arbiter's own
supervision loop re-offers a pending run within its thirty-second backoff, which is what makes
"later" arrive today; a push would buy up to thirty seconds and cost exactly the thundering
herd the consequences section above warns about. Worth building when there is a case it
visibly helps, and the timeline is not it.

## Amendment, 2026-08-20: the two reporting rows, and what "says so" can honestly mean

The table above leaves two rows to a delivery plane that does not exist yet: *nothing can be
placed* ("a run that cannot make its deadline says so; one without stays pending") and *rate
limited* ("wait for the reset if it fits, surface it if it does not"). Both are now built, and
building them settled four things the ADR left open.

**They are one question, asked about two waits.** `Run::prospect_at(at)` is the whole of it:
where will this run stand at the instant the delay it has run into ends? A rate limit says when
it lifts, so that instant is known and the answer can be given an hour *before* the deadline
arrives — at 07:00, a run due at 08:00 whose account frees at 10:00 has already been decided,
and waiting to find out wastes the hour somebody could have used. A fleet that refused the run
says nothing about when it will change its mind, so the instant is `now` and the only thing that
can be established is whether the deadline has already gone. That is not a degenerate case of
the same function, it is the honest one: the alternative is a forecast, and nothing here knows
how long an agent turn takes.

**The fourth appearance of the unspecified-deadline trap, and the one where it would have been
loudest.** `grace_for` must not be shortened by a derived deadline, `review_commitment` must not
give a commitment back for one, and now: a run whose deadline is the moment of submission has
missed it within a second, so a rule that skipped the check would announce *every ordinary run
in the fleet*. The ADR's own wording is the test — one without stays pending — and the trap is
now a `Prospect::NoStatedDeadline` variant rather than a comment, so the check cannot be
forgotten by a caller that only pattern-matches what it cares about.

**Said once, into the run's own log.** The delivery plane is phase 5, and its absence does not
make silence the right answer in the meantime: the log is durable, it is what `offload logs`
reads from any node, and ADR-0010 already says notifications are *fanned out from* the event
stream rather than being a second channel. So the fact goes in as a typed event
(`LogKind::Overdue`, carrying a typed `Waiting`), which is what the delivery plane will pick up
unchanged when it arrives. It is said once per stated deadline, latched in memory on the node
that established it — and a moved deadline earns a fresh answer, because that is the one
intervention that makes the old announcement wrong rather than repetitive. In memory for
attendance's reason: this is a note about having *told somebody*, which belongs to whoever did
the telling, and a restart that says it again is at-least-once in the safe direction.

**And it changes nothing about the run.** This is the hard boundary above, in the one place it
is most tempting to cross: a run that has missed its deadline keeps being offered, keeps its
checkpoint, keeps its lease, and stays exactly as placeable as it was. `offload ps` still says
`pending`. The alarm is a sentence, not a decision — the moment "it cannot make the deadline"
starts cancelling work, a scheduling hint has become the flag that destroys somebody's night of
agent turns, and it does so on a timer they set casually.

What is still not built, and is a feature rather than a field: there is nothing to *do* about a
spent account. The agent is what waits, per-account limits are shared by every node using the
same login (open question #5), and moving the run takes the account with it. Surfacing it is the
whole of the answer until fleet-wide account accounting exists.

## Amendment, 2026-08-20: what building the per-account cap changed

Five things the writing decided, none of which this ADR had settled.

**The ceiling is policy, not capability.** `WorkPolicy::max_concurrent_account`, beside the run
count and the share budget — not a field on the agent capability, where an account's identity
already lives. An account's rate limit is a thing its owner knows and no probe can discover, so
claiming one as a *capability* would be the over-claiming `offload-probe` exists to avoid. It is
still gossiped, because enforcing a shared ceiling means every node on the login has to see it
(wire v13).

**Unstated is not zero.** `None` means "no opinion" and skips the rule. Nobody in this fleet can
find out what an account's real limit is, so a default invented here would refuse work on a
guess — and a guess that refuses is worse than no cap, because the run is then unplaceable
fleet-wide rather than merely slow.

**The binding limit is the lowest anybody claims.** A fold over a field that already has an
owner, so this needs no new row in ADR-0005's table. Minimum rather than maximum or mean because
a node must not be able to raise the fleet's ceiling by claiming more for itself, and because
where two nodes disagree about a rate limit the cautious one is the one to believe.

**It gates starting as well as accepting, which is the opposite of what observed pressure
does** — and the asymmetry is not an inconsistency. What a run waits behind under pressure is
the owner's build: undrainable from here, never promised, so refusing to *start* a committed run
would hold it hostage to a number nobody controls. What it waits behind at an account ceiling is
other runs in this same fleet, on this same login, and they finish. Skipping the start-side check
also makes the cap decorative in the most misleading way available: every node accepts up to the
cap, then starts straight through it, and each one is individually behaving. `Room` carries both
ceilings together for exactly this reason — the first draft had two call paths asking two
different questions, and the narrower one was the one that ran the agent.

**A spent account is *full*, not *pressured*.** So a node on one commits (`Availability::WhenFree`)
rather than deferring, which keeps ADR-0014's promise: the queue ahead of it empties by itself,
and what steers a fresh run towards a node on a *different* account is scoring
(`BidWeights::account_pressure`), which already existed. The exception is a ceiling of zero —
"never" rather than "not yet" — which is a flat refusal, because nothing is going to finish and
make room. One wording change fell out of it: `Availability`'s message says "the run ahead of it"
instead of "its current run", because since this cap the work a run waits behind is not always on
the node that is waiting, and "its" sent somebody to the wrong machine.

**And the cap is best-effort, which is worth saying out loud.** It is summed from gossip, so two
nodes each holding a free slot can decide to start in the same instant and overshoot it. That is
tolerable *here* and nowhere near placement: overshooting costs a rate-limit reply the agent
already reports and this project already handles, while getting placement wrong costs two agents
committing to one repository. The same problem one level down — two fleets over-committing one
device — is why the broker is a ledger rather than an estimate. **Nothing about epochs, leases or
fencing may ever be justified this way.**

The fingerprint that makes any of it possible was the actual blocker, and it was not a design
question: `offload-probe` derived an account label from `$USER` and `$HOME`, which is per-machine,
so `bid::evaluate`'s account pressure was live code that could never match anything. It now hashes
the account uuid the agent itself records, which two machines on one login agree on. Two
consequences worth keeping: the derivation is a **wire format** in the same sense as the fleet
key's, so it carries a versioned salt and a pinned known-answer test; and the API-key path
fingerprints the key's *value*, because the label it replaced was the name of the environment
variable and collapsed every key-authenticated node in the world into one account.

## Amendment, 2026-08-20: what building the device broker changed

**SQLite, not a lock file.** This ADR said "a small file or socket under a well-known path" and
left the mechanism open. The repo has no file-locking primitive, and `unsafe_code = "forbid"`
rules out `flock`, so a hand-rolled file would have needed one written. SQLite's locking is
genuinely multi-process, `rusqlite` was already in the tree, and the ledger gets its own database
with its own `user_version` — emphatically **not** the node's `state.db`, which is per-state-dir
(one per fleet, which is the problem) and whose own docs assert "one daemon writes" twice.

**Per-user, not per-machine.** `$XDG_RUNTIME_DIR/offload/reservations.db`, falling back to a
per-uid directory under the temp dir. A genuinely device-wide ledger would have to be
world-writable, trading ADR-0012's isolation for a permissions problem, and the scenario this
exists for is one person's two fleets on their own laptop. Two users on one machine can still
over-commit it, which is now a stated limit rather than an oversight.

**The row carries the cost, not just the label.** `{run, instance, demand, shares, expires_at}`.
`Demand::shares()` is still the only authority, but the *promising* instance is what calls it: the
premise of the file is several `offloadd` processes, not necessarily the same build, and an older
reader meeting a demand it has never heard of would otherwise have to guess — mis-counting the
machine's budget in exactly the peer the ledger exists to inform.

**Reconciled from the heartbeat, not maintained on the lifecycle paths.** A reservation *is* a
lease, so the heartbeat that renews the run renews the reservation, which is this project's
existing rule rather than a new one. And it is a reconcile — reserve what is held, release what is
not — because the ledger and the store drift for ordinary reasons: a run finishes, migrates away,
or the daemon is killed and restarts to find its own rows from a previous life. A reconcile handles
the cases nobody enumerated; paired reserve/release calls handle the ones somebody did.

**It reaches the *bid*, not only admission.** The first wiring consulted the ledger where a run is
accepted and started, which produced the right outcome by the wrong route: the node bid "starting
now", won, and then held the run on arrival, because a bid built from the fleet's view cannot see
another fleet's runs. `LocalFacts::device_committed` carries the number into `bid::evaluate`, so
the offer is honest when it is made. A fleet still learns only the number — never the other
fleet's runs, repositories or name.

**An unreadable ledger is degraded, not fatal.** `None`, a loud warning once, and the device
over-commits exactly as it did before the ledger existed. The alternative is a daemon that will
not start because another fleet's file is unreadable, which fails towards *no work at all* in
order to avoid failing towards *too much*.

Verified on two daemons, two independently founded fleets, one laptop, one run permitted each: the
first fleet's run ran, the second fleet's was accepted and **held** — one agent on the machine
instead of two, both rows visible side by side in the ledger — and it started by itself when the
first fleet's run finished. One wording change fell out of it, the same one the account cap needed:
"starting when the run ahead of it finishes", because the run ahead of it belongs to another fleet
and "its" pointed at the wrong machine.

## Alternatives

**Keep counting runs, but lower the count.** Costs nothing and makes every device
permanently more conservative than it needs to be — a phone that could serve three watcher
runs is limited to one because an agent run might arrive.

**Declare real resources — CPU, memory, disk — per run.** Precise in principle. Rejected: the
workload is an agent with a shell, so any number it declares is a fiction, and asking the
submitter for it moves an unanswerable question to a person who has even less information.

**Measure only, ignore declarations.** Simpler, and it handles the "typo turned into a
rebuild" case natively. Rejected as the sole mechanism because load is lagging: a node
accepts three runs in the same second, all before any of them registers, which is exactly the
burst that admission control exists to prevent. Declarations gate the burst; measurement
catches the liars.

**One daemon serving all fleets, with an internal scheduler.** Solves cross-fleet capacity
directly and completely. Rejected by ADR-0012: it reintroduces the shared store whose warmth
signals, blob availability and run registry leak between fleets. The broker exists precisely
to share the one fact that is genuinely about the hardware and nothing else.
