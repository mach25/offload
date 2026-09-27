# ADR-0058: A failed task is restarted, not resumed

**Status:** accepted · 2026-09-10 · built and walked in the session that decided it, on two
daemons · settles what `Recovery::Resume` means for the cheap tier (ADR-0019 §2)

## Context

`decide_recovery` answers `Recovery::Resume` for an unattended failed **task**, and has since
before a task could exist. The permission is explicit and was written for the right reason:

> `Idempotent` work needs no transcript — it is re-runnable from its spec

The mechanism behind that answer is `Supervisor::resume`, which refuses a task on its fourth
line:

> it has no checkpoint — there is no conversation to continue

That refusal is also correct, and is a decision rather than a gap: resuming means continuing a
conversation, and its own doc comment says what somebody wants for a failed task is to run it
again, which is `offload run --task`.

So one decision had two mechanisms and only one of them existed. Measured on two daemons walking
phase 8's demo, on a task that exits 2:

```
13:44:27  nobody was watching; resuming it                       resume=1
13:44:27  could not resume it; this attempt still counts
          error=run 01a08b8f725f cannot be resumed: it has no checkpoint —
                there is no conversation to continue
13:44:57  nobody was watching; resuming it                       resume=2
13:44:57  could not resume it; this attempt still counts          (…and again)
```

Three consequences, in order of how much they cost:

1. **The program never ran a second time.** A failed task is not picked back up at all.
2. **The retry budget was spent on refusals.** The tick counts a refusal as a spent attempt
   (deliberately — see the entry in `docs/pitfalls/lifecycle-and-recovery.md` about *not* putting
   `permits` inside `resume`), so `max_resumes` was gone in ninety seconds and the run then
   escalated `TooManyResumes` about a run nothing had ever restarted.
3. **The reports promised what could not happen.** `offload explain` said *"recovery picking it
   up again in 16.7s (try 1)"*, and the daemon's own warning told an operator that a shell script
   had no conversation.

The question this settles is not *whether* a failed task may run again. That was decided, twice,
and both times deliberately: `build_task` stamps `Restartability::Idempotent` on every task, and
ADR-0043/ADR-0054 have a departing node hand a failed task to the fleet precisely so that
**another** node re-runs it from its spec — walked last session. Deciding here that a failed
program must not be re-run would contradict a position the tree already holds and has measured.

What is open is which door, and what it is called.

## Decision

### 1. `Recovery::Resume` is one decision with two mechanisms, and the tick dispatches

The core keeps deciding *whether* to pick a failed run back up, and gains nothing about what
starting one means — that is the ADR-0019 split, and the recovery module is `offload-core`'s most
carefully ordered function. The dispatch is at the tick, on `Work::kind()`:

* `Work::Agent` → `Supervisor::resume`, unchanged.
* `Work::Task` → `Supervisor::restart_task`, which starts the nominated program again from the
  spec.

### 2. `restart_task` is its own door, and `offload resume` still refuses a task

An arm inside `resume` would be smaller by a dozen lines and is refused for the reason the
existing comment gives: `offload resume` is the door a **person** types at, and a door that
quietly means *continue this conversation* for one tier and *run this program again* for the
other is two verbs sharing a word. Two doors, one caller each, both named for what they do.

The counting is the cost, and this file has an entry about exactly that (*"a gate has as many
doors as there are ways to start an agent"*). So it is stated rather than left to be rediscovered:
**there are now two ways to start a task** — `start_run`'s task arm and `restart_task` — and both
go through `launch_task`, whose fence is re-read from the row immediately before the spawn. That
fence, not the number of doors, is what stops two of the owner's programs running at once.

### 3. What a task has no use for is absent, not skipped

`restart_task` keeps the shape of `resume`'s claim — refuse on state, refuse while a program is
still being stopped here, ask the owner's budget, then reopen-and-assign **inside the store's
lock**, because nothing schedules the two doors apart. Everything else is gone rather than
short-circuited: no checkpoint, no session id, no blob fetch, no turn limit. A task has no turn
boundary, so `max_turns` cannot apply to one, and `RunSpec::check` refuses the pairing at
submission.

### 4. The retry that matters is the one the firing source makes

Recovery's backoff is a floor, not the policy. A watcher's real retry is the next tick of its
schedule, the next line from its trigger, or a person typing the command again — each of those a
decision about the world as it is now. Recovery's three attempts are for the case it was always
for: work that stopped for a reason that has since passed.

## Consequences

Good: the cheap tier gets the recovery the decision above it already promised, the reports stop
promising something else, and no new state, gossiped field, schema or wire version is involved —
a restart is an ordinary `Failed → Assigned` transition at a fresh epoch, which every fence
already reads.

Bad, and none of it hidden:

* **A failing task now escalates more than once.** With ADR-0057's notice-bound rule, each
  failure is a new `Notice::Failed` with a new sequence, so a task that fails four times fires the
  escalation four times — sequentially, because a rule runs one occurrence at a time. Before this
  change it fired once, and that was a *symptom*: there was only one failure because nothing
  retried. Measured on the demo staging: one `offload run --task check-api` on a program that
  exits 2 produced **three agent runs on the peer and three pushes to the sink**, all within two
  and a half minutes and all saying the same sentence. Bounded by `max_resumes`, so it is three
  and not unbounded — but three agent runs is real money on a real fleet, and a person watching
  their phone sees one problem three times. Both levers are somewhere else and neither is built:
  the rule's (a notice-bound rule that fires at most once per subject) and the owner's (the
  `TaskConfig` field below). Worth knowing before pointing an escalation at a watcher that fails
  for a whole afternoon.
* **The owner cannot say a nominated program is unsafe to re-run.** `[[tasks]]` has no
  restartability of its own, so a program that sends mail and exits 2 half way through is re-run
  by this pass. That gap is not new — ADR-0043's handover already re-runs it elsewhere — but this
  makes it reachable on one machine, which is where somebody will meet it. The lever is a field on
  `TaskConfig` (the owner is the only one who can answer, the way `agent.max_concurrent` and
  `metered` are nominated), and it is deliberately not built here: nothing has asked for it, and
  inventing the safe default before somebody names a program is how a knob nobody understands
  gets shipped.
* **`Recovery::Resume`'s name is now a tier's worth of wrong**, in the same way `sink` is
  (ADR-0057). Renaming a core variant is a rename of the ordering that took five sessions to get
  right; the doc comment says what it means instead.

## Alternatives

**Let `resume` restart a task.** Rejected in §2: it is the person's door, and its refusal there is
the vocabulary decision ADR-0019 §2 makes about the tier. It would also mean `offload resume` and
`offload run --task` were two spellings of one thing, with `resume` being the one that keeps the
run id — attractive until somebody resumes what they think is a conversation.

**Answer `Recovery::LetGo` for a failed task**, putting it back in the pool as `Pending` and
letting the ordinary placement round re-run it. Tempting because the mechanism exists and is the
one ADR-0043 uses — and rejected on two counts. It loses the retry accounting: `LetGo` spends no
attempt, so a task that fails in 3ms would be placed, fail, and be placed again with nothing
counting, which is the tight loop `max_resumes` exists to bound. And it does not work where most
of this project runs: `cluster.enabled = false` has nobody to offer a `Pending` run to, which is
last session's own finding about a pool with no members.

**Escalate instead — a new `Escalation` saying a program that already ran is not run again.**
Written and reverted inside this session, which is what the tests are for: it contradicts
ADR-0043's handover, where a departing node hands a failed task to the fleet *to be re-run*, and
the test that names that walk failed on the first run of it. The argument it rested on — a program
that failed may have done half its work — is real, and its home is the `TaskConfig` field in the
consequences above, where the owner who wrote the program is the one answering.
