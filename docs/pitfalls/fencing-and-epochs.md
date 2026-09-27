# Fencing, epochs and double execution

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/fencing-and-epochs.md`, same order.

- **Double execution is the failure mode that matters.** Two agents on one repo, both committing,
  is worse than no agent. Prefer a run stalled over a run duplicated.
- **A grant spends an epoch whether or not it is confirmed — and so does a round.** A refused round
  publishes its record at the epoch it reached; otherwise the next round hands the same number out
  again. A round with no bidders spends nothing.
- **An epoch is monotonic per arbiter, which is weaker than a total order.** Two arbiters can issue
  the same number. Three things make fencing real: a grant spends a token even unconfirmed, two
  live grants at one epoch are ordered by **lowest holder id**, and a holder whose record names
  somebody else *at its own epoch* stops its agent.
- **A fence whose failure the caller mistakes for a failed run is not a fence.** `SubmitError::
  LostTheRun` is its own variant: calling `fail` on it writes a terminal state at the *new*
  holder's epoch and makes the run a candidate for auto-resume. Double execution through the fence.
- **A fence after the effect is not a fence, and one whose failure is discarded is not either.**
  Check before the spawn, against the store rather than what the task remembered, and kill the
  agent if the check fails after it started. When a guard and the thing it guards are separated by
  an `await`, the guard is on the wrong side of it.
- **"Stop the agent here" and "the run is over" are two facts.** `Halt::{Cancelled, Superseded}`.
  A leg that merely lost the run writes no terminal state and no `LogKind::Cancelled`.
- **A lease nobody renews is a countdown.** Anything that hands out a lease arranges its heartbeat
  in the same breath.
- **`Orphaned` must never return a live lease**, or an out-of-contact node can act on a run that is
  being moved.
- **Arbitration moves only when the home node is *gone*** — not unavailable, not unknown. A
  `Suspect` node still believes it arbitrates its own runs, so a successor makes two arbiters.
- **A leg that lost a run must not write the run's numbers either.** `Supervisor::holds` (holder
  **and** epoch) is asked by the *writer*, not by each call site — four call-site checks are four
  things to remember and the fifth caller will not.
- **`fail_if_unfinished` must check the holder, not the state.** `Running` is also true of a run
  reassigned to somebody else; testing the state writes `Failed` over the new holder's record.
- **"May I conclude this run" and "may I describe its worktree" are different questions.** A
  finished run has no holder, so `holds` alone calls the machine that just ran it a stranger:
  `describes` uses the epoch to tell "ended here" from "moved away".
- **Two legs assigned from one loaded copy get the *same* epoch, so no fence can tell them apart.**
  `assign` bumps the epoch it read, so a load-modify-save transition run twice concurrently
  computes `old+1` twice, saves twice and launches twice — and every later `holds`, `describes`
  and `fence` check waves both legs through, because both hold the run at the epoch they agree on.
  Fencing protects a run from a *stale* leg; it cannot protect it from two legs born equal. The
  transition has to be read-decide-write under one lock (`Store::update_run`), which is what that
  method exists for and says so.
- **Ask who else calls it.** `Supervisor::resume` had two callers with different intentions — a
  person typing `offload resume` and the recovery tick picking an unattended failure back up — and
  nothing schedules them apart. The state check that makes this safe has to be *inside* the same
  lock as the write; checked only on the caller's copy it is a read of something that can already
  be false.
- **…and `Supervisor::start_run` was that shape too, with a second failure the epoch cannot
  explain.** It did `run.assign(..)` on the copy it was handed and then a whole-row `save_run`.
  A whole-row write does not overrule what landed in between — it **erases** it, and the copy is
  as old as whatever its caller did on the way here. `start_held_runs` snapshots the runs it will
  start and works through them, so every copy after the first is as old as the run ahead of it
  took to start, and `ensure_blobs` — a network fetch of a transcript, which is every migration —
  is an `await` in the middle of that. Measured with a cancel landing in the window: the run came
  back **`Failed`**, restored to `Assigned`, started, and failed by the agent that should never
  have run. `Failed` is a state auto-resume picks up, so the cancel did not merely fail to take,
  it left the run resumable. `Supervisor::hold_here` is the transition now — read, decide and
  write under one lock — and both entrances to holding a run go through it, `start_run` and
  `take_run`'s accept-without-starting. `Store::upsert_run` is `update_run` with the one case it
  could not express: a submission builds a run and starts it in the same breath, so the row's
  first existence is the start's own write.
- **`holder()` is not a lease, and for one state that is the whole difference.** `Run::holder`
  answers `Some(last)` for an **`Orphaned`** run — who *was* holding it, which is the right answer
  to that question and the wrong basis for "may this node hold it". `hold_here` asked it, so a run
  whose lease had been taken away took the already-ours branch: measured, the row stayed
  `orphaned`, the epoch did not move, the agent was registered, and `drive`'s pre-spawn fence
  caught it a cold clone later and reported a lost run to somebody who had not lost one — plus a
  `live` entry that `holder()` then counts against this node's capacity for ever. `Orphaned` grants
  no authority and returns no lease, so the base is `state.lease()`; an orphaned run then goes to
  `assign`, which is legal from `Orphaned` and is the reclaim. The same thing this node does for a
  run *another* node last held, because the last holder's identity is not authority.
- **The row decides, except where the caller is carrying news.** `hold_here` bases on the stored
  row — the only copy that has seen everything that happened while the caller held its own —
  *unless* the row is at a lower epoch, which is what a grant looks like: the arbiter's decision
  travelling, against this node's memory of the leg before it. Reading the row unconditionally
  would refuse migrations and drop the checkpoint they carry, which is the whole of what makes a
  migration one. Both directions are guarded, and both guards were checked by breaking the fix
  and watching them go red.
- **`Run::release` has three callers, and only two of them are the holder.** A drain and a node
  that cannot make the start it promised are the holder changing its mind; `Cluster::hand_over`'s
  give-back is the **arbiter** taking its token back from a node that declined or never answered.
  All three want the same decision out of `let_go_by` — offer it again (ADR-0042) — and only the
  first two know an intention, so nothing may read one out of the field. The walk that found this
  is in `reports-and-cli`.
- **Two legs on one node at one epoch are not a fencing problem, and no fence will ever see them.**
  The epoch is the token for **one grant**, and a second caller arriving after a leg has started
  finds a row saying *this node holds this run under a lease of its own* — which is what the
  running leg put there. `hold_here` waves it through, correctly, and returns the epoch it read.
  What decides whether a second agent starts is `Supervisor::register`, and it used to decide
  nothing: `insert` **replaces**, so the running leg's cancel sender and event stream were dropped
  while the leg carried on, and both legs reached `drive` and built a workspace. Three harms in
  order — two `git worktree add` at one path; the displaced leg becomes **unstoppable**, because
  `cancel_run` finds the other leg's entry; and whichever loses the git race writes `Failed`, which
  is terminal, beats the live record and fences out the healthy leg. The claim is exclusive and
  taken under the `live` lock now (ADR-0051), in `register` rather than in its four callers, and
  `SubmitError::AlreadyGoing` is its own variant so no caller mistakes it for a failed start.
- **A capacity check that excludes the run being granted cannot also be the check that it is not
  already running.** `take_run` excludes it everywhere on purpose — `started_excluding(run.id)`,
  and a `queued` test skipping `held.id == run.id` — because a run arriving now must not be counted
  against itself. So a grant for a run this node is already running went straight to `start_run`.
  Two questions, one exclusion, and the second question was never asked.
- **A filter taken before a loop that awaits is not a check.** `start_held_runs` filters its
  snapshot on `live` and then works through it one slow start at a time; `resume` checks `live` and
  then does two more things before registering. Neither is wrong and neither is enough: the check
  that decides has to be taken *with* the write.
- **"Only one node offers it" is not "only one *pass* offers it", and a draining node runs two.**
  ADR-0042 made exactly one *node* — the arbiter — offer an unheld run, and the loop's own comment
  reads that as the property being safe. `Mesh::drain` calls `place` for the run it has just
  released while `Mesh::supervise`'s tick, seeing the same run `Pending` and unheld, calls `place`
  too; on the departing node those are **one node**, both copies say *N*, and both grant *N+1* —
  which no fence downstream can order, because neither leg is stale. What stopped it being two
  agents was **git**, not a fence, and the leg that collided wrote `Failed` first, so the healthy
  leg lost and a clean migration ended `failed`. ADR-0050 is the fix: `Cluster::place` holds a
  **per-run rendezvous for the whole round**, so the second arrival waits rather than skipping —
  the drain's pass cannot yield, since `Lifespan::Exits` stops everything the moment `depart`
  returns — and then stands down rather than running a second round, returning what the round it
  waited for decided. In `place` rather than in its six callers. Checked by
  `offload-cluster/tests/mesh.rs::two_rounds_for_one_run_on_one_node_are_one_round`.
- **A leg can lose after it has finished, and the rule that records a loss asked for a live
  agent.** `record_run` wrote `Superseded` only when `live` still held the run — so a daemon frozen
  across its agent's last turn thawed, read the agent's result before the gossip, completed the run
  at epoch 1, and had that overwritten by bravo's epoch 2 with **nothing on alpha saying its whole
  leg was not the run's** (measured, session ninety). The merge was right; the record of it was
  missing. `Superseded { finished: true }` is that record, for `Completed` only — a *failed* leg
  picked up elsewhere is ADR-0043's handover and carries its work. Residual: a record that arrives
  already finished names no holder, and the row is not written rather than written with a guess.
