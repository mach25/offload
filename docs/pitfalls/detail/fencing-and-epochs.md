# Fencing, epochs and double execution — full entries

The working rules are in `../fencing-and-epochs.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **Double execution is the failure mode that matters.** Two agents on the same repo, both
  committing, is worse than no agent. Every side-effecting path checks the epoch. When in
  doubt, prefer a run stalled over a run duplicated.

- **A grant spends a token whether or not it is confirmed — and a *round* does too.** The
  within-a-round half of this was fixed by threading one record through the attempts; nothing
  carried it across the boundary. `place` works on a local copy and publishes only a **confirmed**
  grant, so a round in which nobody confirmed left the arbiter's view at the epoch it started
  from, and the next round counted from there and handed the same numbers out again — to nodes
  that, in the swallowed case, were already running under them. Two grants at one epoch from one
  arbiter is exactly what the epoch exists to prevent: `fence` cannot order them, so neither
  agent is ever told it lost, and `merge_run`'s equal-epoch tiebreak is left holding a decision it
  was only ever the backstop for. A refused round now publishes its record at the epoch it
  reached, which is what this node knows — nobody confirmed, so `Pending`, past everything spent —
  and a node that *did* take one of those grants is then behind the fleet and stops. Only when the
  epoch moved: a round with no bidders spent nothing, and inventing a record for a submission the
  operator is about to be told was refused would leave it in the view holding nothing.

- **An epoch is monotonic per arbiter, which is weaker than a total order.** Two arbiters both
  compute `next()` from the same record and issue the *same number* to different nodes, and
  `fence` only refuses a caller that is *behind* — so both were admitted, and at equal epoch
  `from_holder` was true on both sides, meaning the merge believed whoever spoke last and
  flipped again next tick. Neither agent was ever told. ADR-0002 named fencing as what makes a
  split-brain grant a *rejected* writer and, two paragraphs later, called the reconciliation
  last-writer-wins; both sentences were in the file. Three things make the first one true: a
  grant **spends a token whether or not it is confirmed** (a decline is not proof it never
  arrived — ADR-0006's silence-is-a-decline makes "took it, answer lost" the ordinary case, and
  that node runs under the very epoch the next one is handed), two live grants at one epoch are
  ordered by **lowest holder id** so every node picks the same winner offline, and a holder
  whose record names somebody else **at its own epoch** stops its agent. And it needs no
  partition: a few missed probes, or one arbiter re-offering within a round, is enough.

- **…and a fence whose failure the *caller* mistakes for a failed run is not one either.**
  `drive`'s two guards work — neither spawns an agent for a run this node has lost — and `launch`
  then treated the refusal like every other error from `drive` and called `fail`. That uses
  `abandon`, unfenced on purpose because a run whose workspace could not be built has no holder to
  fence against; for this one error it is exactly backwards. The row by then is the **new
  holder's**, at the new holder's epoch, so `Failed` is terminal, beats the live record everywhere,
  and `note_failure` makes the run a candidate for auto-resume: somebody else is running it, the
  fleet says it failed, and recovery is entitled to start a second agent. Double execution, reached
  through the fence firing. `SubmitError::LostTheRun` is its own variant so the caller cannot
  confuse the two — and the reason it survived is that the test drives `drive` directly and never
  goes near the function that calls it.

- **A fence *after* the effect is not a fence, and a fence whose failure is discarded is not
  one either.** `drive` did both: it spawned the agent and *then* called `Run::started`, whose
  epoch check it ignored with `if ….is_ok()`. Everything before a spawn takes time somebody
  else can act in — a cold clone runs for minutes and restoring a workspace fetches blobs from
  peers, which is exactly the slowness that gets a node concluded dead and its run reassigned —
  so an agent that had lost its run kept going, on the same repo, while somebody else resumed
  the same conversation elsewhere. The check is before the spawn now, against the *store*
  rather than what the task remembered, and a failure after it kills the agent it just started.
  The general shape: when a guard and the thing it guards are separated by an `await`, the
  guard is on the wrong side of it.

- **"Stop the agent here" and "the run is over" are two facts, and one signal cannot carry
  both.** The cancel channel was the only way to take an agent down, so everything that came down
  it was recorded as `Cancelled` — including a node that had just *lost* the run. That node
  stopped its agent (right) and then wrote a terminal state into the record `record_run` had a
  moment earlier overwritten with the new holder's copy: unfenced, because an operator's cancel
  always wins, at *their* epoch, and terminal, so it beat the live record everywhere `absorb`
  does not refuse a peer's word about a run it holds. The fleet said cancelled, the agent worked
  on, and the operator was told their run had stopped. `Halt::{Cancelled, Superseded}` is the
  distinction, and the same rule decides what goes in the *log*: `LogKind::Cancelled` is terminal
  to every follower, so a leg that merely lost its run writes nothing.

- **A lease nobody renews is a countdown.** `Run::renew` sat with no callers while
  `supervise` orphaned on `lease_expired`, so a run granted to a peer went `Orphaned` a minute
  later and was reclaimed by its holder's next gossip — invisible, self-correcting, and wrong.
  Anything that hands out a lease has to arrange for its heartbeat in the same breath.

- **`Orphaned` must never return a live lease.** If it does, an out-of-contact node can act
  on a run that is being moved, and the whole fencing story collapses.

- **Arbitration moves only when the home node is *gone*.** Not when it is unavailable, and
  not when it is merely unknown. A `Suspect` node can refute, and meanwhile it still believes
  it arbitrates its own runs — so a successor that jumps in on a suspicion makes two arbiters,
  two bid rounds, and one run granted twice. `arbiter_for` prefers a stalled run to a
  duplicated one, which is the same trade every epoch check makes.

- **…and the rule covers the *numbers*, not just the states.** A leg that had lost a run still
  ran `refresh_workspace_summary` on its way out, publishing its own stale checkout as the run's.
  Progress gossips and `accept_progress` breaks a tie on the author's clock, so the last write
  wins the fleet — the same arithmetic that had `offload rm` telling everybody a checkout on
  another machine was gone, in the column somebody reads to find out whether there is uncommitted
  work. `Supervisor::holds` (holder **and** epoch) is the question all four of these paths were
  getting wrong in different words — and it is asked by the **writer** now rather than by each
  caller: `fail` refuses a run this node does not hold, so the next path somebody adds cannot
  forget. Four call-site checks are four things to remember, and the fifth caller will not. The arm
  in `launch` stays for the two things a guard cannot do — an honest log level, since "run failed"
  is the wrong sentence for a leg that lost a race, and letting go of the process handle.
  `Supervisor::recover` keeps its own copy because it runs at startup before any of this exists,
  and says so.

- **…and the node that says so has to be the one it belongs to.** `fail_if_unfinished`'s
  `still_ours` tested the run's *state* — `Running` or `Checkpointing` — which is a question about
  what an agent would be doing and not about whose run it is. A reassigned run is `Running` too,
  on somebody else's machine, so a leg whose agent crashed at the moment it lost the run wrote
  `Failed` over the new holder's record: unfenced, at their epoch, terminal, winning everywhere
  while their agent worked on. Reachable by ordinary luck rather than an arranged race —
  `record_run` asks the agent here to stop, and an agent that dies of its own accord first makes
  the pump report `Finished` rather than `Halted`. It checks the holder and the epoch now, which
  is what the name always claimed.

- **"May I conclude this run" and "may I describe its worktree" are different questions.** A
  finished run has no holder — the lease goes with the terminal transition — so a holder check
  alone calls the machine that just ran it a stranger: no final worktree summary, and an audit row
  claiming a write was *refused*, which is a fence somebody would go looking for. `holds` is the
  first question and `describes` the second, and the epoch is what tells "ended here" from "moved
  away" where the holder cannot, because the terminal transitions are fenced.
- **Two legs assigned from one loaded copy get the *same* epoch, so no fence can tell them apart.**
  `Supervisor::resume` loaded the run, checked its state, spent two full `list_runs` scans on the
  capacity check, then called `Run::assign` on its own copy and `save_run`. `assign` sets
  `self.epoch = self.epoch.next()`, so two callers loading before either saves both compute
  `old+1`, both write it, both `register`, and both `launch` an agent into the same worktree. The
  part that makes this worse than an ordinary lost update is that it defeats the mechanism built
  to catch it: fencing separates a stale leg from a current one, and these two are *equal* —
  every subsequent `holds`, `describes` and epoch check passes for both. Two agents committing to
  one repo, which `CLAUDE.md` names as the failure that matters most.
  `Store::update_run` exists for precisely this and its doc says so — "the window does not need an
  `await` in it to be real: two tokio tasks on one store are enough" — and `resume` was the
  load-modify-save it warns about. Found by asking the question the previous seam suggested: what
  else does a person and a machine both call? Two production callers, `offload resume` and
  `recover_failed_runs`, and nothing sequences them.
  **The tripwire did not trip, and that is worth knowing.** Two hundred aligned attempts on a
  `tokio::sync::Barrier`, on a four-thread runtime, before the fix: not one lost update. The
  loser's `load_run` landed after the winner's `save_run` every time and it was refused with "it
  is assigned, so somebody is already holding it" — a photo finish this machine wins, not a
  guarantee, and the sort of thing a slower disk or a busier scheduler decides differently. So the
  fix is structural rather than a timing argument, and the test is kept as an invariant check
  (exactly one resume starts an agent, and the loser is told which) with its own comment saying it
  passed before the change too. A guard that cannot fail today is worth keeping and worth
  labelling; it is not evidence.

- **…and `Supervisor::start_run` was that shape too, with a second failure the epoch cannot
  explain.** Session thirty-four's await-audit named it and left it, for two reasons that both
  turned out to be real: a fresh submission has no row yet — `submit` calls `build` and then
  `start_run` — so `Store::update_run`, which reads first and gives up when there is nothing,
  cannot express the transition at all; and `save_run` writes the whole row, so the grant path's
  semantics had to be preserved deliberately rather than inherited.

  The epoch half is `resume`'s bug verbatim: `assign` on the copy the caller handed in, then a
  whole-row `save_run`, so two callers loading before either saves both compute `old+1`. The half
  that is *not* about the epoch is the one that reproduced. **A whole-row write does not overrule
  what landed in between — it erases it**, and the handed copy is as old as whatever its caller
  did on the way here. `start_held_runs` snapshots the runs it will start and then works through
  them, so every copy after the first is as old as the run ahead of it took to start; `start_run`
  has exactly one `await`, `ensure_blobs`, and that is a network fetch of a transcript — which is
  to say every migration.

  Measured, through the real loop with the fetch gated: a held run cancelled inside that window
  came back **`Failed`**. Not "the cancel did not take" — the row was restored to `Assigned`, the
  agent was launched, and it failed on its own. And `Failed` is the state auto-resume picks up, so
  a run somebody deliberately stopped was left resumable. The `started()` fence one function down
  cannot catch this: it compares the epoch against the row, and the row is the thing that was
  clobbered back into agreement.

  The fix is `Supervisor::hold_here` — read, decide and write under one lock — with `take_run`'s
  accept-without-starting going through the same function, since it is a whole-row write of a copy
  that crossed a network and could equally put back a run this node has since finished. The base
  is the **stored row**, *unless* the row is at a lower epoch, which is what a grant looks like:
  the arbiter's decision travelling against this node's memory of the leg before it. Basing on the
  row unconditionally refuses migrations and drops the checkpoint they carry. Both directions have
  a test and both were checked by breaking the fix: the cancel test went red before it (`failed`
  where `cancelled` was expected), and the grant test went red with the lower-epoch arm disabled.

  `Store::upsert_run` is the new primitive — `update_run` plus the row that does not exist yet —
  and it hands the closure whatever is stored, `None` when nothing is, and writes what the closure
  returns. An error writes nothing, which is the point of handing it the stored copy at all.

  **Walked on a daemon, because it is the transition every start takes**, with a fake agent and no
  model spend. All three arms: a submission with no row (started, completed, clean); a `--queue`d
  run placed by the bid round, where the grant arrives one epoch ahead of the local `Pending` row
  (started, completed, clean); and a run accepted at capacity, held at epoch 1 with "waiting for a
  slot", then started by `start_held_runs` when the run ahead of it finished. Plus the operator's
  case: a held run cancelled stays `cancelled`, gets no worktree, and is never started.

- **`holder()` is not a lease, and for one state that is the whole difference.** `Run::holder`
  reads `state.lease()` and then adds one arm — `RunState::Orphaned { last, .. } => Some(last.node)`
  — which is right for the question it is named after (who *was* holding this) and wrong for the
  one `hold_here` was asking (may this node hold it now). So an `Orphaned` row whose last holder
  was **us** took the already-ours branch and returned `run.epoch` without assigning. Measured with
  a probe over all four ways into that decision: `start_run` returned **`Ok`**, the row stayed
  `orphaned`, the epoch went 1 → 1, and the run was registered as live. Nothing after that works —
  `fence` reads `state.lease()` too, and `Orphaned` has none, so `drive`'s pre-spawn check refuses
  with `act on / orphaned` and reports `LostTheRun` after a workspace prepare, which for a cold
  clone is minutes, to an operator who had not lost the run to anybody. And the `live` entry
  outlives it, which is harmless only for a run somebody else holds: `started_excluding` gates on
  `holder() == me`, and for this one state `holder()` says yes — so the slot stays spent and
  `held_but_not_started` skips the run for ever. The same wedge as the entry above, through the
  door `holder()` opens.

  The fix is the base: `state.lease()`, so a lease of ours takes the branch and everything else
  goes to `assign` — which is **legal from `Orphaned`** and is the reclaim path, spending a token
  as any assignment does. The guard drives both directions in one loop, because they are one rule:
  an orphaned run is reclaimed here whether this node or another was the last holder, since the
  last holder's identity is not authority. It goes red on the first direction — `orphaned` where
  `assigned` was expected.

  Reachability, since two other rules already stand in the way. The local node never orphans its
  own run: `supervise` returns `HeldHere` for `holder == view.local`. And `speaker_decides` refuses
  a peer's `Orphaned` about a run this node holds a **lease** on, which is the sibling of this rule
  and a good one. What is left is the epoch, because a higher-epoch record is taken by `merge_run`
  before any of that reasoning runs: an arbiter that granted to us at epoch N, concluded us gone,
  and orphaned at N can reach a local row still at N-1. A delayed grant at N then lands in
  `take_run` against a stored `Orphaned{last: us}` at N — not lower, so the row is the base. Narrow,
  and the sort of thing a partition produces rather than a keyboard.


- **`Run::release` has three callers, and only two of them are the holder.**

`supervisor.rs`'s call is the holder giving a commitment up — a drain, or ADR-0006's node that
cannot make the start it promised. `place.rs`'s is the arbiter's give-back inside `hand_over`:
`assign` spent a token, the grant was declined or went unanswered, and the token has to be
returned at a number the refused node can no longer act under. Same transition, opposite ends of
the wire.

It matters twice. The epoch arithmetic is the same either way — `assign` then `release` is two
bumps, which is why a refused round leaves the run two past where it started — but `let_go_by` is
not: on the give-back it names a node that never held the run. The consequence, and the fix, are
in `../reports-and-cli.md`. The rule to carry is narrower than either: **when a transition is
reused from the other side of the wire, check every field it writes, not only the one it was
reused for.**


- **"Only one node offers it" is not "only one *pass* offers it", and a draining node runs two.**

ADR-0042 removed `Mesh::owed` so that exactly one node — the run's arbiter — offers an unheld run,
and the loop that does it says so in its own comment: *this is the only offerer, which is the
property that matters*. True of nodes. False of the node it is written on.

`Mesh::drain` releases the run and calls `Cluster::place` itself. `Mesh::supervise`'s tick
snapshots the view a moment later, sees the same run `Pending { let_go_by }` with no holder, and
calls `Cluster::place` too. On a departing node those are one node, so the arbiter is racing
itself — and `hand_over` bumps the epoch on the copy it was handed, so two rounds that both read
*N* both grant *N+1*. Every fence downstream waves both legs through, because fencing protects a
run from a **stale** leg and neither of these is stale. It is `Supervisor::start_run`'s bug
(read-modify-write run twice concurrently) one layer up, in a place nobody had looked because the
comment said the property held.

Found while walking the checkout sweep, which it has nothing to do with. Staging: submit to alpha
while it is the only node, start bravo, wait for the checkpoint to replicate, `offload drain`
alpha. Two captured hits:

```
alpha  handed over               run_id=… name=bravo starting=starting now
alpha  an unheld run was placed  run_id=… name=bravo                          +1.4ms
bravo  cloning repo mirror  ×2, 7.4ms apart, and two concurrent `git worktree add`
audit  granted to 73f8f863 at epoch 3        (twice)
```

**What stopped it being two agents was git, not a fence.** The second `worktree add` failed with
`already used by worktree at …`, which is the only reason the walk noticed at all. And the outcome
is worse than a coin flip: the leg that *collided* wrote `Failed` first, so the healthy leg —
workspace ready, transcript restored — was fenced out by the loser's write, and a clean migration
ended as a run `failed` with a raw git error on it.

**And the collision is incidental, which is the part worth carrying.** Re-measured as a control
before fixing it — forty-eight passes, sixteen with the supervise tick at one second (all clean) and
thirty-two at `probe_interval_ms = 200` (one hit, two aborted on machine load) — the hit came out
**silent**: `handed over` and `an unheld run was placed` 1.78ms apart, two `granted to 3e453757 at
epoch 3` rows, and then *nothing*, because bravo committed rather than starting (`starting when one
of the 2 runs ahead of it finishes`) so there was one clone and no second `worktree add`. One
arbiter spent one token twice and no log anywhere said so. The staging's hit rate is also load- and
cadence-dependent rather than a property of the bug: at a one-second tick it did not reproduce in
sixteen passes at all.

Two dead ends worth not re-walking. **A re-read of the run before `place` is not the fix**: it is
this file's favourite idiom, and two rounds that overlap exactly still both read the same copy, so
it narrows the window rather than closing it — the same objection this file already makes about a
guard separated from its effect by an `await`, and a whole bid round is an `await`. **And the
drain's pass cannot simply yield to the tick**: under `Lifespan::Exits`, `main` stops every agent
the moment `depart` returns, so a drain that stood aside for "the tick, later" hands the run to a
tick that never runs again — which is exactly the stranding ADR-0042 exists to have ended.

ADR-0050 is the decision: `Cluster::place` takes a **per-run rendezvous for the whole round**, so
the loser waits rather than skipping and then stands down rather than running a second round,
returning what the round it waited for decided. In `place` rather than in its six callers, because
the two that raced were not the two anybody would have guessed and the seventh caller will not
remember either. Checked by
`offload-cluster/tests/mesh.rs::two_rounds_for_one_run_on_one_node_are_one_round`, which fails with
`[(RunId(…), Epoch(1)), (RunId(…), Epoch(1))]` when the rendezvous is bypassed.

- **Two legs on one node at one epoch are not a fencing problem — and the git error is what proves
  they were concurrent.** This is the entry session fifty-two left open as an observation with no
  mechanism: twice in eighty daemon passes, alpha built two worktrees for one run 1ms apart off a
  single grant, one `git worktree add` failing with `cannot force update the branch 'offload/<run>'
  used by worktree at '<the same path>'`, writing `Failed`, and fencing out the healthy leg.

  **The wording is the evidence.** `WorkspaceManager::prepare` runs `git worktree add --quiet -B
  <branch> <path> <commit>` and passes **no `--force`** — and without it git's `die_if_checked_out`
  fires first and says something else. Measured directly, outside this codebase, on git 2.55: a
  worktree added earlier gives `'X' is already used by worktree at 'X'`; a directory renamed away
  and not pruned gives the same; **two adds at once give `cannot force update the branch`**, from
  `create_branch`, because `worktree add` is a read-decide-write too and the loser passes the
  checked-out test before the winner registers and dies inside the branch update after. 200 forced
  pairs: 17 `cannot force update`, 182 `is already used`, 1 torn `worktrees/<name>/commondir`. So
  the session fifty-two logs are not a stale checkout and not anything upstream in placement: they
  are two `prepare` calls at once, which is two `drive` tasks, which is two legs past `register`.

  **And no epoch could have told them apart, because they were both entitled to it.** `Run::assign`
  ran once. The second caller found a row saying *this node holds this run under a lease of its
  own* — what the running leg wrote — so `hold_here` took the already-ours branch, correctly, and
  handed back the epoch it read. Everything downstream (`fence`, `holds`, `describes`) then waves
  both through. The one place that could have refused was `Supervisor::register`, and it `insert`ed
  unconditionally: `BTreeMap::insert` **replaces**, so the running leg's `LiveRun` — its cancel
  sender and its event stream — was dropped while the leg itself carried on. Three harms, in the
  order they land: two `worktree add` at one path; the displaced leg becomes **unstoppable**,
  because `cancel_run` and `request_checkpoint` both look up the live entry and after the
  replacement the entry belongs to the other leg; and whichever leg loses the git race writes
  `Failed`, which is terminal, beats the live record everywhere, fences out the healthy leg, and —
  through `note_failure` — makes the run a candidate for auto-resume.

  **The callers were the only guard, and they did not agree.** `start_held_runs` filters its
  snapshot on `live`, before a loop that awaits a blob fetch per run. `resume` checks `live` and
  then does two more things before registering. `submit` does not check and does not need to, its
  id being new. And `take_run` **cannot** check with what it has: every capacity question it asks
  excludes the run being granted — `started_excluding(run.id)`, and a `queued` test skipping
  `held.id == run.id` — because a run arriving right now must not be counted against itself. That
  exclusion is right for capacity and silent about *"is it already running here"*, so a grant for a
  run this node was already running went straight to `start_run`.

  ADR-0051 is the decision: `register` returns `Result`, refuses when `agent_here`, and takes the
  check and the insert under one `live` lock — in the writer rather than in the four callers, for
  the reason `Supervisor::holds` and ADR-0050's rendezvous are. `SubmitError::AlreadyGoing` is its
  own variant so a caller cannot mistake it for a failed start, and `start_run` claims **before**
  it records the acceptance or writes `preparing`, so a leg that will not start leaves nothing
  behind. The refusal logs at `warn` with **both epochs**, because which two callers met is still
  not known: equal epochs are two legs born of one assignment, different epochs are a re-grant
  landing on a run this node already runs. Checked by
  `offload-node/src/supervisor.rs::a_second_start_for_a_run_already_going_here_starts_no_second_agent`,
  which holds a leg open inside `ensure_blobs` — the only `await` in `start_run` — and calls
  `start_run` again with the row the first leg wrote. Without the refusal the second call registers
  over the first and walks into the blob fetch, where it waits on a gate only the first leg opens:
  the timeout is the assertion, and the red reads *the second start went past the registration and
  into the blob fetch*.
- **A leg can lose after it has finished, and the rule that records a loss asked for a live
  agent.** Found reading the audit rows only a fence writes, which no session had seen on a screen
  (session ninety). Staging: two daemons under `setsid --fork`, a run on alpha, `kill -STOP` on
  alpha's daemon three seconds in, bravo granted the run at epoch 2 after 45–70 s, then `kill
  -CONT`. The fake agent is its own process and keeps running through the freeze. With a 400 s turn
  the agent is still live on thaw, `record_run` sees `live` holding epoch 1 under a record naming
  bravo at 2, halts it and writes `superseded … had not finished a turn`. With a **50 s** turn the
  agent exits inside the freeze; on thaw alpha's pump reads the result before the gossip lands,
  records a checkpoint at turn 1 and completes the run at epoch 1, then bravo's `running`@2
  arrives, `live` is empty, and `save_run` overwrote alpha's completed row with nothing written
  anywhere — `offload audit` on alpha showed `granted` and `accepted` only, while alpha's own log
  said `finished after 1 turn(s)` and its branch held a commit that is not the run's. The merge was
  correct (bravo's leg completed at 2 and is the run); the record of the loss was missing. The fix
  loads the stored row before the save: `Completed` at a lower epoch, and `RunProgress::by` naming
  this node, is a finished leg that lost, written as `Superseded { finished: true }` (serde default,
  so older rows read as the running case). `Failed` is excluded on purpose — ADR-0043's handover
  picks a failed leg up elsewhere and carries its checkpoint forward, which is not a loss. Checked
  by `supervisor::tests::a_leg_that_finished_and_then_lost_is_written_down`, with a control whose
  leg ran on bravo. Residual: a record that arrives already `completed` names no holder, and the row
  is not written rather than written with a guess.
