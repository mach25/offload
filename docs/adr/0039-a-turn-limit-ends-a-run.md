# ADR-0039: A turn limit ends a run, and nothing may give it another turn

**Status:** accepted · 2026-08-28 · amended 2026-08-29 (§4, below) · supersedes nothing

## Context

`RunSpec::max_turns` was declared in phase 1, initialised to `None` in eight fixtures, and read by
no production path in any of the seven phases since. Phase 1 recorded the gap in as many words —
"the CLI has no `--max-turns` flag because the agent has no such option; the supervisor must count
boundaries and stop the run itself" — and nothing changed. Re-measured here on **claude 2.1.251**:
there is still no `--max-turns`, so the agent cannot be asked to stop itself.

That makes it the third field of a shape this repository has now named twice:
`WorkPolicy::allowed_agents` was enforced and settable by nothing, `bid_delay` was settable and
enforced by nothing, and both read to a reader as a control being applied. `max_turns` was neither
half — declared, gossiped on every record, and inert.

It is also the only bound in a spec that stops an agent that is *making progress*. A deadline
changes when the fleet gives up waiting and never what the work may do (ADR-0013); a constraint
decides where a run lands; an allowlist decides what it may touch. Nothing bounded how many turns
a model decides a task deserves, and the answer to "how much did this misunderstood prompt cost"
arrived on a bill.

## Decision

1. **The limit is checked at the turn boundary, and reaching it ends the run.** ADR-0004's
   boundary is the only place an agent can be stopped without discarding a turn's work, and the
   check is `turns >= limit` on turns *completed*: a run that has finished its tenth turn under a
   limit of ten may not open an eleventh. One predicate, `RunSpec::turn_limit_reached`, returning
   the limit rather than a `bool`, because every caller has to say it.

2. **Counted over the run's whole life, not per leg.** Turns are already offset across legs so a
   resumed agent's turn 1 is the run's turn 7, and the limit reads that number. `RunProgress::asks`
   made this argument first: a run that migrates four times gets the budget its submitter agreed
   to, not a fresh one from every node it lands on.

3. **It ends `Failed`, with the limit in the reason.** Not `Completed`, which is terminal *and*
   successful — an abandoned run that reads as a success is the failure `CONTINUE_PROMPT` exists
   to avoid, and it would be this one every time. Not `Cancelled`, whose `by` names the node an
   operator typed `offload cancel` at; there is no such node, and inventing one would put a lie in
   the only field that variant carries. Not `Pending`: a checkpoint hands a run *back to the
   pool*, so a spent run released that way is one the next bidder carries on past the limit — the
   cap applied on one machine at a time and never to the run.

4. **The capture at the limit is unconditional.** Whatever `[checkpoint] every_turns` says,
   including `0`. A run somebody capped is precisely the run they want to look at, and ending it
   on whatever checkpoint it last happened to take — or on none — throws that away. A capture that
   *fails* still ends the run: the limit is a decision to stop, and carrying on to spend an
   unauthorised turn because the save did not work is the wrong way round.

5. **Nothing may give it another turn.** This is the half that is easy to leave out and the half
   that makes the rest worth having. `Failed` is exactly the input `decide_recovery` turns back
   into a running agent, so a limit enforced only where it is reached is one the recovery tick
   undoes a backoff later — unattended, for as many resumes as the policy allows.
   `Escalation::TurnLimitReached` is checked **above attendance**, beside `NotOursToRetry` and for
   the same reason: `Unattended` is the branch that resumes, and a run that quietly spent its
   budget overnight is unattended by construction. A check below attendance is one the case it
   exists for walks straight past.

6. **An explicit `offload resume` is refused too.** The tempting reading is that a person at the
   keyboard is present, and presence is what `Attendance` lets decide elsewhere. It does not apply:
   attendance decides whether to act *without being asked*, and this is somebody asking for one
   more turn than they said they wanted. A limit a command can walk past is not a limit.

7. **Zero is refused, in the type.** `Option<NonZeroU32>`, so serde refuses it on the way in and no
   path can forget to check — including a rule stored months ago and fired tonight by a build that
   never saw the CLI's parser. This is `allowed_agents`' empty list: a value that reads as a
   restriction while meaning "do nothing at all" is two other sentences said badly ("don't submit
   this", "leave the flag off"), and the keyboard is the only place the difference can still be
   asked about.

**Rejected: making the limit editable.** A spent run's only way forward is a new run, because
`max_turns` is fixed at submission. Raising it would need a third `SpecEdit` field, which
`RunSpec::notify` already says belongs to an ADR of its own rather than to whoever needs it first.
Recorded as the residual below rather than taken here: the argument for the third editable field
should be made on its own, and "the field I just built needs it" is the weakest possible version
of it.

## Consequences

* `--max-turns N` on `offload run` and on `offload when`. It is worth more on the latter: a rule
  fires unattended for months, and a prompt that was fine in January against a repo that has since
  changed is exactly the case where a bound on the work is the only thing that notices.
* `offload ps` shows `4/10` where a limit was asked for. A control nobody can see is
  indistinguishable from one not being applied, which is the state this field spent seven phases
  in.
* **Residual: a capped run reports no cost.** `cost_micro_usd` comes only from the agent's final
  `result` event, and a stopped agent never emits one — so `offload ps` shows `-` for the run whose
  spend somebody capped, which is the run they most wanted the number for. Pre-existing and wider
  than this ADR (a released checkpoint, a drain and a cancel all end without a result), but a
  turn limit makes it *certain* rather than incidental. It is phase 7's "cost and token accounting
  per run" item, which now has a symptom to point at.
* **Residual: no way to raise a spent limit.** Decision 6 leaves "start a new run" as the only
  route, and that loses the conversation. If a third `SpecEdit` field is ever argued for, this is
  the first candidate.

## Amendment, session thirty-two: §4 was true of the code and false of the runs

Decision 4 says the capture at the limit is unconditional, because "a run somebody capped is
precisely the run they want to look at, and ending it on whatever checkpoint it last happened to
take — or on none — throws that away". The capture was unconditional as written. It was also
**failing two times in three**, so the outcome §4 forbids was the usual one.

The cause is the seam between this ADR and the guard added hours later in the same session, which
refuses a capture whose transcript holds no conversation. That guard is safe everywhere else
because a failed capture is retried at the next boundary — and this is the one caller that stops
*on* the boundary it captures at, so there is no next one. Neither half was wrong; nobody walked
them together.

Measured, real agent, `--max-turns 1`: two of three runs ended with no checkpoint line in
`offload explain` and a dash under SAFE, while the TOKENS column beside it was populated — the
same transcript, read once before the agent was stopped and once after, disagreeing about whether
the conversation was there. The fix is `await_transcript`: wait up to three seconds for the agent
to write the turn it just spoke, *then* stop it, *then* capture. 8 of 8 afterwards.

Two things worth keeping. The first attempt stopped the agent and then captured, on the premise
that the file is final once the process is gone — true, and not sufficient: a `SIGTERM` inside the
window loses the message outright rather than flushing it, which is why the wait has to come
before the stop. And §4's other sentence — "a capture that *fails* still ends the run" — is
unchanged and still right; what changed is how often it has to.

**Residual, unchanged:** a capture can still fail for reasons a wait cannot fix, and the run still
ends. What it no longer does is fail for the reason that was almost guaranteed.
