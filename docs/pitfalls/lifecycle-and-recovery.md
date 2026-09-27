# Daemon lifecycle: drain, shutdown, restart, recovery

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/lifecycle-and-recovery.md`, same order.

`offload drain` told **eight** separate lies over **five** sessions. They are listed first because
each fix produced the next one — except the last three, which nothing about the command produced:
the sixth came from a fix to a *different* command, the seventh had been true since the flag was
invented, in the one path nobody thought to check it from, and the eighth (last in this file) was
true since the day a second **kind of work** existed. Every fix was correct; what kept being missed
was a different question each time — how many callers, how many doors, and now what kind of work.

- **It said "stop accepting" and never did**, so a laptop about to be closed went on bidding and
  winning. Refused at the **bid** *and* at the **grant** — a grant can arrive after the bid that
  earned it — and reported by `offload status`. One-way and in memory: a drain is a departure.
- **On a fleet of one it did nothing at all**, because the flag was set inside `Mesh::drain` and a
  node with no cluster never takes that arm. The flag belongs to the *handler*, and the refusal had
  to reach the no-cluster `submit_run` path. A draining node may still **submit** — the laptop
  asking the desktop to work is what this product is for.
- **It said "nothing to hand over" about a run it had just stopped.** `left` came from
  `held_count()`, a *capacity* question, and a released checkpoint has no holder by design. Count
  outcomes as they go (`mesh::Drained`).
- **The fix for the fleet-of-one lie left `SIGTERM` accepting work**, because the flag moved into
  one of `Mesh::drain`'s **two** callers. `mesh::depart` is the only way in and `Mesh::drain` is
  private. A fact hoisted into "the caller" has as many owners as that function has callers.
- **A drain waits minutes, so it streams** (`Response::Draining`, ADR-0035). `DEFAULT_ASK_PATIENCE`
  and `drain_deadline_secs` are both 300s, so the step that matters carries the `tool_use_id` —
  that is the difference between information and the way out. `Report::silent()` logs the same for
  the shutdown path.
- **It reported the good outcome as the worst one.** `wait_for_checkpoint` returned `None` for
  three reasons including *the run finished by itself*. `Boundary::{Reached, Finished, StillMidTurn,
  Gone}`; `Gone` stays in `left`, because unknown is not good news.
- **Two daemons on one state directory both start and nothing looks wrong.** `statedir` locks the
  directory; `bind` *connects* before removing a socket that looks like a leftover. Two different
  mistakes, two mechanisms.
- **An agent outlives the daemon that spawned it, and fencing does not reach a process.** A
  leftover agent makes no writes, so no epoch check stops it. `leftovers` is a node-local durable
  note, swept at startup **before** `recover`, carrying the session id because pids are reused; a
  pid whose `/proc` entry cannot be read is **not** signalled.
- **A run only fails if something says so.** `pump` ends when the stream closes; an agent that dies
  without a `Result` produces no transition and its holder keeps renewing the lease.
  `fail_if_unfinished` is the answer. Any event-driven state machine needs an answer for the stream
  simply stopping.
- **A node's own runs need tending whether or not there is a fleet** — `tend_own_runs`, not the
  gossip tick, which does not run on a fleet of one.
- **Autonomy in failure follows attendance, never the deadline.** Unattended resumes itself;
  attended stays broken in front of the person. **Unknown is not unattended** — a restarted daemon
  leaves the run for a person.
- **A fresh retry budget must not be a side effect of a deletion.** `stop_recovering` on the human
  path only; the auto-resume path writes the count it spends.
- **A limit enforced only where it is reached is undone by whatever restarts runs.** `max_turns`
  stops the run at the turn boundary; `Failed` is exactly what `decide_recovery` turns back into a
  running agent. Checked **above attendance**, because `Unattended` is the branch that resumes and
  a run that spent its budget overnight is unattended by construction. Three enforcement points,
  one predicate (`RunSpec::turn_limit_reached`): the boundary, the recovery decision, and
  `resume`.
- **A spent run must end, not go back in the pool.** A released checkpoint hands a run to the next
  bidder, who carries on past the limit — the cap applied one machine at a time and never to the
  run. `Failed` with the limit in the reason: not `Completed`, which reads as success, and not
  `Cancelled`, whose `by` would have to name a node nobody typed at (ADR-0039).
- **A refused request must not have the side effects of the successful one.** `stop_recovering`
  ran *before* `resume`, so a refusal cleared the entry too — and it removes it outright, so the
  run left the recovery bookkeeping for good: no `recovery` line in `offload explain`, and the
  tick never looked again because it iterates that map. Now on the `Ok` arm. Not mechanically
  checked — the handler needs a socket and a `Ctx`, and there is no harness for it.
- **A report that names a cause has to be told when it stops being the cause.** `attendance` said
  "— which is what decided" unconditionally, which was true while attendance was the only thing
  that could decide. It now lists the escalations that *are* about attendance, so a reason added
  later has to claim this deliberately rather than inherit it.
- **A request must survive a failed attempt to honour it.** `offload checkpoint` armed a flag that
  was read *and cleared* at the next boundary — so a capture that then failed consumed the
  request, the run carried on to completion, and the operator who was told "it will be taken at
  the next turn boundary" got silence. Read without clearing; clear on the arm that honours it.
  Reachable before by any capture failure, and easy the moment a lagging transcript became one.
- **A drain's sixth lie, and the first one nothing in the command could have caught.** `offload
  checkpoint` and a drain arm the *same* flag, and it is read without clearing — a standing
  instruction until honoured. So the request outlives the deadline that stopped waiting for it: a
  run still mid-turn when the drain gives up finishes its turn, captures, and **releases itself**
  minutes after being reported as still here. Nothing then offers it, because a checkpointed
  `Pending` run is one `supervise` reads as parked by a person. Measured: released twenty seconds
  after the drain returned, with its checkpoint already copied to the peer that was bidding 57 to
  take it, and never offered. `Mesh::owed` finishes the handover at that boundary (ADR-0041).
- **Only one of `depart`'s two callers can promise anything about later.** `offload drain` stays
  up; a signal is the process going away, and `main` stops the agents the drain could not move as
  soon as it returns. `Lifespan::{StaysUp, Exits}` is a parameter and **not** a read of
  `Report::silent()` — "nobody is listening" and "this process is about to stop existing" are two
  facts, and inferring one from the other is how the departure flag ended up in the wrong place
  twice.
- **This node is a holder like any other.** A decision about "has somebody else taken this run"
  written as `run.holder().is_some()` answers *still ours, mid-turn* and *somebody else took it*
  identically, and they are opposite instructions — the first says wait, the second says stop. Pass
  the local id in and compare. Caught by the guard on the first run of it, which is the argument
  for the decision being a pure function (`fn owed(run, me)`) rather than three conditions inside a
  pass that needs a `Cluster` to test.
- **The drain's seventh lie: a drained node starts agents.** `stop_accepting` gates the **bid**
  and the **grant**, and auto-resume asks neither — so `recover_failed_runs` went on spawning
  agents on a node whose own `offload status` said `accepting no — drained`. Measured: drain
  returned 11:42:50, the run failed 11:43:23, and at 11:43:54 the same node logged `nobody was
  watching; resuming it` and `spawning claude code`, while a peer holding a **replica of its
  checkpoint** sat idle. Fourth path this one flag has been missing from. `Circumstances.departing`
  is checked **above everything, including standing** — every other answer asks whether a retry
  would *work*, and this one asks whether this is the machine to try it on.
- **A start that gives up has to give the registration back — and the halt somebody put in it.**
  `start_run` registers the run as live before it can fail, because the registration is what a
  concurrent `cancel_run` has to find, and `launch` is what installs the arm that cleans up
  afterwards. Everything between the two returns early: nothing ran the cleanup, so a migration
  whose transcript could not be fetched left an entry in `live` that **nothing ever removes**.
  That is not untidy, it is a wedge — `held_but_not_started` filters on `live`, so the run never
  comes round again, and `started_excluding` counts live-or-`Running` for a run this node holds,
  so the slot stays occupied by an agent that was never started. And the second half is worse than
  the first: `halt_agent` takes the cancel channel, signals, and answers "its agent was stopped"
  **writing nothing**, because the writer is `drive` — so handing the registration back with a
  halt still in it drops the cancel, leaves the row `assigned`, and lets the next tick start a run
  somebody was told had stopped. `Supervisor::unregister` takes the entry and the sender under one
  lock, so the race has two outcomes and no third: either the sender is still here and no cancel
  can ever be accepted, or somebody has it and their message is on its way, which is what bounds
  the wait for it.
- **The `live` entry outlives the run on purpose, and what it holds must not.** `Supervisor::
  release` drops the agent handle and nothing else, because `drive` calls it *before* writing the
  terminal state and the run's last log lines are appended after — and because `describes` asks
  whether a finished run ended **here**, which only this leg's epoch can answer. Nothing removed an
  entry at all, so every run this daemon ever started kept a `broadcast::channel(256)` whose ring
  is allocated on creation: measured in-process at **~37 KB a run**, and on a daemon at **~26 KB a
  run** of resident growth reclaimed, reproducibly, over 100 runs. ADR-0020 is what makes that a
  leak rather than a rounding error — a rule firing every three seconds is 1,200 runs an hour, on
  something meant to run on a phone. `close_stream` lets the channel go at the end of `launch`,
  which is after *every* way the leg can end including the `fail` arm, and a follower still
  attached drains what is buffered and then sees the close, which for `logs -f` is the run ending.
  `forget_legs` takes the entry itself when the run's **row** is pruned, a leg being a fact about a
  run; `prune_spent_records` returns what it deleted so the caller can, rather than dropping the
  list on the floor.
- **"An entry exists" and "an agent is going here" are not one question, and `live` has never
  answered both.** A `LiveRun` outlives the leg that made it — `release` drops the cancel handle
  before the terminal write, `close_stream` drops the channel after the last append, and the epoch
  stays for good because `describes` has nothing else to tell a run that ended *here* from one that
  ended elsewhere. Three facts, three lifetimes, and the entry's presence answers all three at once
  and for as long as the longest of them. Five readers asked presence. The one that cost something:
  `hold_here` does not touch `live`, so a run granted back to a node that had already run a leg of
  it landed beside the old entry and `awaits_a_slot` read it as already started — the run sat
  `assigned` under a renewed lease with no agent, for ever, holding the slot it would never use.
  Measured 3m32s on an idle node reporting `runs 1/2 · accepting yes`, against 12s to start on the
  fix. `agent_here` is `cancel.is_some()` for "has it started" (`awaits_a_slot`,
  `release_unstarted`, `started_excluding`), `stream_open` is `live.is_some()` for "may a follower
  still get more" — one step later, because asking for the agent ends `logs -f` a few lines early.
  **Ask the field.**
- **A refusal *now* is not an answer about later, and a drain released the run before it asked.**
  ADR-0041 gave that rule to a run still mid-turn at the deadline; a run that reached its boundary
  *inside* the deadline and was refused by every peer was dropped — so the run that behaved better
  got the worse outcome. And "dropped" is not "left here": a released run is `Pending` with a
  checkpoint and no holder, which `offload_core::supervise` reads as **parked by a person**
  (ADR-0014) and passes by, and nothing else offers it either. Measured: drain with the peer down,
  `offload explain` then describing the run as waiting for `offload resume` — about a run nobody
  parked — and both daemons restarted idle, accepting and holding the checkpoint with the run still
  `pending`. Now a refusal is the same debt a missed boundary is (`unplaced`), and only
  `Lifespan::Exits` strands it, because `main` stops the agents as soon as the drain returns.
  Handed over 23s after the peer appeared. **`Pending` + a checkpoint is two facts with one
  spelling; if you add a third way to reach it, `supervise` will read it as the first.**
- **…and the answer was a field on the run, which retired the debt that found it.** `Pending` plus
  a checkpoint is two facts with one spelling, and the entry above fixed the half a process could
  reach. The other half is everything that outlives one: a drained node **restarted** before the
  fleet has room, and `Lifespan::Exits` — a `SIGTERM`, which is what closing a laptop looks like —
  which recorded nothing at all. Measured on the old build through the `SIGTERM` door: released
  and refused at 07:07:15, daemon restarted 07:07:17, still `pending` at 07:08:32 on an idle
  accepting node, `explain` saying it *waits for `offload resume`* about a run nobody parked. On
  the fix, placed one second after the restart and resumed at turn 3. `RunState::Pending` carries
  `let_go_by: Option<NodeId>` (ADR-0042) — **inside the state**, so it inherits the state's owner
  and arbiter and cannot go stale, since every way out replaces the state and every way in states
  it afresh. `None` is the conservative reading and today's behaviour: a person may be coming
  back, so wait for them. Two things follow that are easy to get wrong. **The reason travels with
  the request**, because the only moment that knows is `request_checkpoint`, a turn earlier —
  `GivenUp::{Parked, LetGo}`, and `LetGo` **wins** when both arrive, since the node is leaving
  whatever else was typed at it. And **`Mesh::owed` had to go**, not merely stop being needed:
  leaving it would have put the departing node and the run's arbiter in a bid round for the same
  run, which is the two-grants-at-one-epoch case `place` says the epoch cannot fix.
- **…and a failed run had the same shape, one state further on.** ADR-0034 stopped a drained node
  spawning agents for its own failed runs and answered every departing case with
  `Escalation::NodeIsDeparting`, which ended the question one step early: *this node will not
  start an agent* and *nobody should* are two facts, and a drain is where they come apart. The run
  is resumable, unattended, inside its budget, past its backoff — and it stayed `Failed`, which is
  terminal, so no arbiter offers it and no node bids, while its checkpoint sat **replicated on the
  peer that would have taken it**. Measured: `offload drain` said "nothing to hand over", and the
  run was still failed after a wait and a restart of both daemons, by then with no reason on
  screen at all. `departing` now *wraps* the run's own verdict instead of replacing it —
  `Resume`/`Wait` become `Recovery::LetGo` and everything else keeps the reason that is true of it
  wherever it goes (ADR-0043). Three rules fall out. **The effect is a transition, not a start**
  (`Run::let_go`: `Failed` → `Pending { let_go_by }`), so every line of placement is ADR-0042's
  and none of it is new — the fact needed nowhere to live because the run stops being failed.
  **Only where the fleet could start it**: `Checkpoint::is_durable`, else `NoCopyElsewhere`, since
  a run offered without its conversation is left `Pending` behind an unreachable checkpoint, which
  is worse than the `Failed` it came from. And **the guarantee is now structural**: `Resume` and
  `Wait` are the only answers that spawn an agent and neither leaves the departing match, swept by
  a property test — where before it was a claim about line order.
- **A departing node's own pass is not the recovery tick's, and the record has to leave with the
  goodbye.** Two things the walk found and neither is optional. `Mesh::drain` filters terminal
  runs out of `held`, so it has never seen a failed one — and a node whose only run has failed
  holds nothing, so that pass returns on its first line; the tick would get there a second later
  on the `offload drain` door and there is no later on the signal door, because `main` stops
  everything the moment `depart` returns. The sweep goes in `depart` for the reason
  `stop_accepting` does. Then, measured with the sweep and without the second half: the node wrote
  `Pending` to its own store, exited, and every peer held the run as `Failed` until that daemon
  came back — the stranding moved from the record to the network. `announce_departure` carries
  `Gossip::runs` and is already waited for, so publishing into the view before `depart` returns is
  the whole fix: 265ms from `SIGTERM` to placed on another machine.
- **And a fifth path, for a reason one step stronger than the drain's.** A **revoked** node's
  auto-resume asks neither the bid nor the grant either, so the two doors ADR-0044 shuts leave the
  one that spawns agents. `Circumstances::revoked` is checked above everything including
  `departing` — every other input asks whether a retry would *work*, `departing` asks whether this
  is the machine to try it on, this asks whether the machine is allowed to be one — and it does
  **not** become `LetGo`, because letting go is an offer to a fleet a revoked node cannot reach.
- **A `bool` that arrives among small integers is a call site nobody can read.** `decide_recovery`
  went to eight positional arguments, four of them numbers, and clippy's limit was the useful
  accident: `Circumstances` names every field at every call site instead. It matters most for the
  two callers that must agree — the recovery tick and `offload explain` — which were otherwise two
  argument lists lined up by eye.
- **…and a sixth: the one a person types.** `offload resume` starts an agent exactly the way a
  submission does, and its handler asked only the queue — so a drained node, a node whose owner
  said `accept = "never"`, and a **revoked** node whose run ADR-0044 had just halted each resumed
  it at their own socket and went on taking turns (ADR-0047, measured three times). One
  `server::hosting_refusal` now, called by that door and by the no-cluster submission arm.
  Counting the doors is the lesson: this is the fifth clause-shaped fix on the same gate, each of
  them correct, none of them asking what *else* spawns an agent.
- **…and a seventh, which is the one nobody types at.** The tick did not ask the owner's work
  policy, so a node reporting `accepting no — network is metered and policy disallows it`, whose
  submission *and* resume doors both refused, logged `nobody was watching; resuming it` sixty
  seconds later and ran the agent on the metered link. `Circumstances::hosting` now, beside
  `departing` and behaving like it — `Resume`/`Wait` become `LetGo` because a node that will not
  host is still one the fleet can hear — with its own escalation where nobody else holds a copy,
  because "this node is leaving" about a laptop sitting there at 15% is the wrong cause
  confidently stated (ADR-0049). **Not** `permits` inside `Supervisor::resume`, which is the
  edit that suggests itself: the tick counts a refusal as a spent retry, so a battery dip would
  burn the run's budget on a condition that is nothing to do with the run.
- **…and a sentence has as many places as it has callers.** ADR-0049 gave the handover its second
  cause and fixed the two places the old sentence was looked for — the escalation and `offload
  explain`. The walk it had said it could not do then found the third: `handing the failed run
  back to the fleet` was logged as **"this node is leaving"** about a laptop sitting right there
  on a metered link. Same mistake as the door count, one layer down, and found the same way —
  by running it.
- **A gate has as many doors as there are ways to start an agent, and the last one found is the
  one that opens by itself.** Seven now: the bid, the grant, the no-cluster submission arm's
  drain clause, the tick's `departing`, the tick's `revoked`, the policy clause on both doors a
  person types at, and the tick's. Every fix was measured, correct and written up; what kept
  being missed was the counting. The answer to "is it fixed" is not the answer to "how many
  doors are there".
- **…and the eighth thing about that gate is not a door, it is which cause it names.**
  `stand_down` sets the revoked latch **and** the draining one (ADR-0044), and `hosting_refusal`
  asked the drain first — so every door it guards told an evicted device's owner to *restart
  `offloadd` to take work again*, advice that cannot work on a machine the fleet has thrown out.
  `status` had the opposite ordering and the reasoning beside it the whole time. Both orderings
  refuse, which is why it survived every test that asserted a revoked node takes no work; the
  entry is in `reports-and-cli`, because what was wrong was the sentence and it was found by
  pairing a report with the door. `revoked_refusal` is the one place now, and it falls back to
  the constant rather than to `None` when `fleet.json` cannot be read.
- **A function that asks "could anybody *else* do this" has to check that anybody else exists.**
  `fleet_could_start` decides whether a departing node hands a failed run to the fleet or keeps
  it, and its own name is the question: for a `Resumable` run it asked `is_durable` — does another
  node hold a copy — and for an `Idempotent` one it answered **yes unconditionally**, because such
  work is re-runnable from its spec and there is nothing to fetch. True, and half the question:
  somebody has to be there to re-run it.
  Measured on one daemon with `cluster.enabled = false`: two tasks that exit 2, a `SIGTERM`, a
  restart, and both came back `pending` at epoch 2 with `last assigned to` set — released into a
  pool with no members. Nothing ever offered them again, *including the node that came back a
  second later*, because offering is the cluster's job. `offload explain` was exactly right about
  it (*"this node offers it to the fleet until somebody takes it"*, and *"nobody was asked to
  take it: this node is not in a mesh"*), so nothing was hidden — what was wrong is that nobody
  ever would, and a **task cannot be resumed**, so `offload resume` was not even the way out.
  The second consequence is the one that bit: `rule_run_in_flight` reads *non-terminal* as
  in-flight, so a stranded occurrence **jammed its rule permanently** — every later firing
  dropped, silently, on a plane whose whole job is to be loud.
  `Circumstances::alone` is the missing input, and it is deliberately **any node this one has
  met** rather than any node answering *now*: a peer in a bag comes back, and a `Pending` run
  waiting for it is what ADR-0043 wants. `ClusterView::alone` is the one definition, asked by the
  recovery tick, by a departure and by `offload explain`. The refusal is its own escalation
  (`NobodyElseInTheFleet`) rather than `NoCopyElsewhere`, because telling somebody with one
  laptop that their *task's* conversation exists nowhere else is a true sentence about a thing
  that has no conversation — and a confident wrong answer about why nothing happened.
- **…and the tick needed it too, which is why the fix is not only in the drain.** `LetGo` is
  reachable without departing: a node whose owner will not let it host (ADR-0049) releases a
  failed run the same way, so a laptop under its battery floor on a fleet of one strands runs
  with the daemon still running. `tend_own_runs` now takes the cluster for that **one** fact —
  the fleet's *size*, never its opinion, which is what keeps the pass out of the gossip loop.
- **…and that second cause needs its own sentence, which makes it a 2×2 rather than a pair.**
  Leaving-or-refusing crossed with no-copy-or-no-second-device is four sentences, and the first
  version of this fix had three: the owner-refused corner borrowed `OwnerWillNotHost` — *"no other
  device holds a copy"* — for an idempotent task that **has no copy to hold**, which is
  `NodeIsDeparting`'s deleted mistake arriving from the other axis. `OwnerWillNotHostAndAlone` is
  the fourth, and the two halves are fixed by different actions: plug this one in, or add a second
  device. The caller computes the pair in one `match` so a new cause cannot pick up an old
  cause's words.
- **The owner-refused corner cannot be walked on a single machine, and that is a property of the
  gate rather than a gap in the walk.** `permits` refuses on `accept`, charging, the battery
  floor, a metered link and `allowed_agents`; the first and last are **config**, read once at
  startup, and the middle three come off the probe. So the hosting gate can only disagree with
  the placement gate that already let the run start if a *capability changed underneath it* —
  unplugged, or tethered — which is exactly the premise of ADR-0048 and ADR-0049 and is not
  scriptable without root. Attempted: `accept = "always"`, a task that exits 2, then a restart
  under `accept = "never"`. It proves nothing about this corner, for the reason in the next
  entry, and the enforcement is the unit test — said here so the next session does not read the
  walk as covering it.
- **The recovery tick is keyed on an in-memory map, so a restart neither strands a failed run nor
  recovers one.** `recover_failed_runs` iterates `self.recovery`, which a new process starts
  empty; the run is not considered at all, and `offload explain` says so honestly (`resume
  refused here right now`, no decision reported). Worth knowing because it is easy to reach for a
  restart as the cheap way to re-drive this pass — it is not one — and because it says where the
  danger actually was: the **drain**, which runs the pass with the map still populated and
  `departing` set, which is the path the stranding was measured in.
- **One decision, two mechanisms, and only one of them existed: a failed task was never
  restarted.** `decide_recovery` answers `Recovery::Resume` for an unattended failed task — right,
  and written before a task could exist — and the arm that acts on it called `Supervisor::resume`,
  which refuses a task on its fourth line for want of a conversation. Both halves correct in
  isolation. Measured on two daemons: `nobody was watching; resuming it` then `could not resume
  it; this attempt still counts` in the same millisecond, twice more thirty seconds apart, and the
  program never ran again — while `offload explain` promised *"picking it up again in 16.7s (try
  1)"* and the log told an operator a shell script had *"no conversation to continue"*. Because a
  refusal counts as a spent attempt (deliberately — see the `permits`-inside-`resume` entry above),
  the run's whole retry budget went on refusals and it then escalated `TooManyResumes` about a run
  nothing had started. `Supervisor::restart_task` is the task tier's door (ADR-0058), dispatched at
  the tick on `Work::kind()`; `offload resume` still refuses a task, because that door is the one a
  person types at. **Adding a tier does not only add columns to reports — it adds a second meaning
  to every decision that says what to do next.**
- **…and the fix that suggests itself contradicted a decision the tree already held.** The first
  attempt was an escalation — *a program that already ran is not run again by itself* — with a good
  argument behind it: a task that exits 2 may have sent half its mail. It broke four tests on the
  first run, and the one that named its own walk is why: ADR-0043 has a **departing** node hand a
  failed task to the fleet precisely so another node re-runs it from its spec, walked one session
  earlier. `Restartability::Idempotent` is the tree's answer and this pass is not the place to
  overturn it. Where that argument does belong is a field on `TaskConfig` — the owner who wrote
  the program is the only one who can say — and it is unbuilt because nobody has asked. **Before
  deciding a mechanism is missing, find the place that already decided it was not.**
- **`offload checkpoint` is the fourth agent-only path, and the only one that was never given a
  door.** `prepare_start`, `resume` and `drive` all ask for the agent half and refuse a task with a
  reason; `request_checkpoint`'s only guard is `cancel.is_some()` — *something is running here* —
  and `start_run` registers that channel one branch **above** the split on `Work::Task`, so a task
  walks through a guard written for an agent. Measured: `offload checkpoint <a running task>`
  answered *"it will be taken at the next turn boundary"*, the run went to `checkpointing`, and
  `offload explain` said *"finishing its turn before checkpointing"* about a `/bin/sleep`. Nothing
  was ever captured. The guard is in `Run::request_checkpoint` — core, where the run knows its own
  kind — because two callers guarding is two things to remember. `TransitionError::NoBoundary`
  rather than `WrongState`: the state was `Running`, which is exactly when this is legal, so a
  sentence naming the state would send somebody to wait for a moment that is never coming.
- **…and the costly half was the drain, which armed the same flag and then waited for it.** A
  running task sat in the drain's boundary wait for the whole of `drain_deadline_secs` — **300
  seconds by default** — and was then counted in `later`, which promises a handover *at the run's
  next turn boundary*. Measured with the deadline cut to 20s: 20s of waiting on one `/bin/sleep`,
  then the promise. Now partitioned before the wait, with `Drained::no_boundary` beside `later`
  rather than inside it: 20s → **0s**, and the mixed case (one agent run, one task) waits for the
  agent alone. Nothing is owed for a task — the node stays up and the program ends, or the process
  goes and ADR-0043 re-runs it from its spec. The fleet-of-one arm counts from `held_count()`
  rather than from that partition, so it needed splitting too (`held_no_boundary`), or one fact
  would have had two spellings — the trap `finished`, `later` and `pooled` were each split out of
  `left` to avoid. **`offload drain` has now been wrong about something in five separate sessions;
  every fix was correct and none of them asked what other *kind of work* it was holding.**
- **A stop that returns at the signal is not a stop, when a start can follow it.** `offload_stop`
  (ADR-0070) first sent the shutdown signal, emptied the "one daemon per process" slot and
  returned, while the daemon was still draining. An iOS app brought back from the background
  starts the daemon again at once, and that start found the slot empty and ran a second daemon
  on the same state directory and socket. The slot now holds the thread, and stop joins it before
  returning. `start_serves_the_socket_and_stop_ends_it` asserts the socket refuses the moment stop
  returns, and fails without the join (control-run, session ninety-two). Found while writing the
  Swift caller, not by a walk.
- **An absolute path inside an iOS app's container goes stale on every reinstall and update.** iOS
  moves the container. The iOS app's `node.toml` held the old `state_dir`, and the daemon created
  that directory afresh (the Simulator allows it) and minted a new identity there. The device
  silently became a different node, outside its fleet. The app re-points the container paths on
  every launch (`Paths.repoint`, ADR-0070's amendment). A daemon that finds no `node-key` mints one,
  and it cannot tell "a new device" from "the right device looking in the wrong place". Anything
  that computes a state dir has to compute it fresh each time.
- **A host app at targetSdk 35 draws edge to edge whether or not the layout expects it.** The
  Android app's button row sat under the status bar from its first build. Nothing noticed, because
  every walk drove the app through adb and nobody pressed a button, until one had to be pressed to
  make the approval key. The fix is `NoActionBar` plus system-bar insets as padding. The next
  build still hid a button on the phone: a phone held upright is narrower than five buttons, so
  "Key" sat past the right edge. The row now scrolls sideways. Check a host app on the narrowest
  device before trusting its layout. Find a host
  app's buttons by screenshot, not by the bounds `uiautomator` reports: those said the button was
  where the status bar is.
- **Anything a host app calls on the platform's behalf can throw, and an uncaught throw takes the
  whole app down.** `BiometricPrompt.authenticate` threw `SecurityException` for the missing
  `USE_BIOMETRIC` permission inside `onResume`, which crashed the app with the daemon's foreground
  service in it. A failure to prompt now writes `<id>.refused`, so the waiting command gets an
  answer and the app stays up.
- **UniFFI reads its metadata from the library's symbols, so generate the bindings before
  stripping.** `scripts/build-android-product.sh` first generated from the stripped `.so` in `dist/`.
  It produced no Kotlin at all, without an error, and the Gradle errors that followed pointed at
  missing imports. It now generates from the unstripped library in `target/` and fails if no `.kt`
  appeared. Separately, a UniFFI error field called `message` collides with Kotlin's
  `Throwable.message`, and the generated file does not compile. `MobileError::Daemon` uses `reason`.
