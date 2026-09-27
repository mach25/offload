# ADR-0029: A node knows when its account's limit lifts, and holds work until it does

**Status:** accepted · 2026-08-27 · wire **v23** · builds what `AgentEvent::RateLimit` already
claimed · supersedes nothing

## Context

Offload has always counted **concurrency** per account — `WorkPolicy::max_concurrent_account`,
folded to the lowest any node on the account claims, keyed on the fingerprint ADR-0028 makes the
owner's to choose. What it has never known is the thing an operator actually runs into: **the
account's own usage limit**. A plan has a five-hour window and a weekly one, nothing on the machine
states them, and no amount of per-node accounting sees them coming.

The signal exists and has existed since phase 1. Claude Code emits a `rate_limit_event` per turn
with a status, a limit type and a reset instant, and `AgentEvent::RateLimit`'s doc comment says:

> Feeds per-account bid scoring; this is the real signal the probe's account fingerprint cannot
> provide.

It fed nothing. `judge_rate_limit` used it for exactly one thing — deciding whether *that run* was
going to miss its deadline — and then dropped it. Nothing remembered it, so the next run was
started into the same wall: an agent spawned to stall, holding a session slot, on a node that
looked idle in every report. This is the "dead code that asserts a mechanism" entry with the
polarity reversed — the mechanism is asserted in a comment and the code was never written.

## Decision

### 1. The node remembers a blocking limit **that stated when it lifts**

`Supervisor::limits`, keyed by the agent's own `rateLimitType` (`five_hour`, `weekly`, whatever it
invents next), holding the instant each lifts. Four rules, each of them a rule from somewhere else:

* **Only a *blocking* status.** `rate_limit_blocks` already exists and already draws this line:
  the agent sends one of these every turn, and `allowed_warning` is a quota report rather than a
  hold-up. Treating those as limits would hold every run in the fleet on a routine message.
* **Only where it said when.** A block with no stated reset is **not remembered**. A fact with no
  expiry is the thing that gets stuck, which this codebase has learned twice — the drain flag that
  deliberately does not expire because "a drain is a departure", and a route that is *away* versus
  one that is *gone*. The run that hit it is affected regardless; the ones after it must not be
  held on a wait with no known end.
* **Latest wins per kind**, which is not "most recent wins". The agent restates the same limit
  every turn, so an earlier reset for a kind we already hold is the same fact told again; moving
  the value backwards would let a run start into a limit still in force.
* **In memory, and forgotten on restart.** That is the safe direction: the next turn's event says
  so again within seconds, and the other reading is a daemon holding work on a limit that lifted
  while it was down.

`account_limited_until()` is the read, and it drops expired entries on the way past. The map is the
whole state, an entry that has lifted is not a fact, and so there is no tick that has to remember
to clear anything — which is the property that made the walk's last step work with nothing running.

### 2. It gates **starting**, and that is not the pressure rule

"Load gates accepting, never starting" appears to forbid this and does not. That rule is about a
number **nobody controls** — the owner's build on the same machine, undrainable from here and never
promised — so holding a committed run behind it would be holding it hostage. A rate limit is the
other kind: it lifts at an instant the agent stated, nothing outside this fleet has to happen
first, and the run it would hold is one whose agent would otherwise be spawned only to stall on the
same limit. **That is being full, with a clock instead of a count.**

So the check lives on `Room::for_one_more`, beside the two ceilings — and, after the walk, the
*value* is put there by `Supervisor::room` rather than by the caller. It was originally passed in at
each site, and the fleet-of-one submit path hands a bare `Capacity` in, which `From<Capacity>` fills
with `None`: the gate did not exist at all on the path most of this project's testing uses.
Measured — a node rate-limited for another 76 seconds started the next run immediately. That is the
four-call-sites mistake in a fifth set of words, and the funnel is also the right *owner*: the limit
is this node's own observation, so a caller supplying it was a caller repeating us.

### 3. The bid says *when*, not how far back — `Availability::NotBefore`

A rate-limited node **commits** rather than refusing, like any full node, because refusing leaves
nobody holding the run (ADR-0014). What it reports is the instant and not a queue position:

* An instant, because the agent said it. `Availability::WhenFree` carries a count precisely
  because an ETA there would be fabricated; here refusing to say the time we were *told* would be
  the opposite mistake.
* Not `WhenFree` with a number, because the two send somebody to different places: one empties
  when work *here* finishes, the other empties when a clock elsewhere says so.
* **It outranks a queue when both hold.** A slot frees long before the run can use it, so naming
  the count would send somebody to watch the wrong thing.

`is_now()` is false for it, so `winner()`'s existing precedence — able to start now, then score,
then lowest id — already steers the run to a node on an unspent account, with nothing added.

**Wire v23**, and this one earns it where three recent fields did not: it is a new enum *variant*
on the bid exchange, so a v22 node cannot decode the offer at all rather than dropping a number it
does not understand. That is the difference between "your `ps` is less informative" and a bid lost
in silence.

### 4. What is printed, and where the clock is

`offload status` grows a line **only while a limit is holding**, because it answers the otherwise
unanswerable "the machine is idle and my run has not started":

```
account     acct:0c0c0c0c0c0c0c0c  ·  from /home/owner/.claude-alt
            └─ rate-limited: new runs start in 39.0s
```

The refusal an operator meets is rendered where there is a clock
(`supervisor::describe_refusal`), and `offload-core`'s own sentence carries no instant at all. That
is not fastidiousness: `Millis`'s `Display` renders a *duration*, so the first version of the status
line printed the reset instant as **`496117104h5m`**. A crate with no clock cannot format an
absolute time, and pretending otherwise produces exactly that.

## Walked, on two daemons and two accounts

The claims above about a *fleet* were unverified when this ADR was written, which is worth saying
rather than tidying away — the fleet-of-one path refuses rather than commits, so the commitment and
the steering were both reachable only with peers. Walked afterwards, `alpha` on
`acct:0d0d0d0d0d0d0d0d` and `beta` on `acct:0e0e0e0e0e0e0e0e` (ADR-0028's nomination is what made
two accounts on one machine possible to arrange at all):

* **The commitment.** With beta away, `offload run` printed `run 01a042095675, starting when the
  account's rate limit lifts`; `ps` said `assigned`, one agent had launched, not two.
* **The lift.** `there is room now; starting the run held for it`, from the tick that already polls
  for a freed slot, and the run finished normally. Two agent launches for two runs.
* **The steering.** Both nodes up, submitted on alpha: `run 01a0420ce70c — accepted by beta`, and
  beta's agent log confirms it ran there. No code in the comparison — `is_now()` is false for
  `NotBefore`, and `winner()`'s existing precedence did the rest.

Two things the walk found, each fixed in its own commit and neither about this decision:
`offload explain` had no answer at all for a held run (its canvass line said `already holds this
run`, and its footer said `No node would take it right now` about a run alpha had accepted), and the
gossiped worktree summary said `waiting for a slot` about a machine with four free slots.

**What it also settled, by not being a problem.** A commitment held by a rate limit is *not* handed
to an idle peer, because `review_commitment` acts only on a *stated* deadline — the same rule that
stops every undeadlined run in the fleet bouncing from queue to queue. Measured and left alone: run
four sat on alpha while beta was free, which is what a full node's commitment does too, and changing
it is an ADR rather than a patch.

## Consequences

* A node whose account is spent stops starting agents into the limit, and starts the run it is
  holding the moment the limit lifts — from the tick that already polls for a freed slot, because a
  poll cannot miss the moment.
* With peers, a run is steered to a node on an account that has quota left, by the precedence that
  was already there.
* **The fleet does not learn it.** A peer sharing the account finds out for itself, one turn later.
  Gossiping it was considered and is the tempting half of this: the fact even has a natural home
  (the agent capability already carries the `AccountId`) and the gossip body is JSON, so it would
  cost no bump. It is left out for attendance's reason (ADR-0013) — an account's position is a
  *moving* value, and a gossiped snapshot hands a reader a number from before the silence and calls
  it current. The half that would be safe to travel is exactly the half with a stated reset, since
  that is a fact with an expiry rather than a snapshot; whether that is worth a field is a decision
  for the session that has two rate-limited daemons in front of it, not this one.
* **Nothing is inferred about how much quota is left.** Only "blocking, until then" is recorded. A
  fraction the agent does not report is a number this project would be inventing, and
  `offload-probe`'s standing rule is that an unverifiable fact is not claimed.
* The owner-declared side is untouched: `max_concurrent_account` is still how somebody says what
  the account may run at once. A *spend* budget — dollars per day — is cost accounting and stays in
  phase 7.
