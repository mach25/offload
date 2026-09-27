# Daemon lifecycle: drain, shutdown, restart, recovery — full entries

The working rules are in `../lifecycle-and-recovery.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **`drain` said "stop accepting" in its own doc comment and never did.** It handed its runs
  over and returned, so a laptop about to be closed went on bidding and winning — measured, one
  second after `offload drain` reported "nothing to hand over", and the run it had just moved to
  the desktop could come straight back. Refused at the **bid** *and* at the **grant**, because a
  grant can arrive after the bid that earned it, and reported by `offload status`: a node that
  silently takes nothing looks exactly like a broken one. One-way and in memory, because a drain
  is a departure — the way back is starting the daemon again, and a flag that expired would be
  machinery for a state that lasts as long as somebody's walk to the door.

- **…and the fourth lie: on a fleet of one it did nothing at all.** `Request::Drain` branches on
  whether there is a cluster *and* a mesh, and `stop_accepting()` was the first line of
  `Mesh::drain` — the arm a node with `[cluster] enabled = false` never takes. Measured: `offload
  drain` printed `nothing to hand over`, `offload status` said `accepting yes`, and a run submitted
  a second later was accepted and started. The comment on that very line records the previous
  session finding the identical failure one level *lower* and fixing it for the early return
  ("the version that set no flag at all meant `offload drain` on an idle laptop did *nothing
  whatsoever*") — the same sentence describes what was left. Two faults, both live, so fixing
  either alone still leaves a broken drain: the flag is now set by the **handler**, before the
  branch, because "this node is leaving" is a fact about the node and not about the handover pass;
  and the refusal it turns on had to reach the path a fleet of one actually takes, since
  `is_draining` was checked at the **bid** and the **grant** and a fleet of one has neither.
  `submit_run`'s no-cluster arm had moved the *grant* check down to itself and not the drain one,
  ten lines from an `evaluate` that checks drain first of all and says why. Deliberately **not**
  hoisted above the branch: a draining node may still *submit*, because the door's grant is
  `{Submit, Deliver}` and hosting is separate — a laptop being closed asking the desktop to work is
  the thing this product is named for. And two silences are two facts (ADR-0034): the report told a
  fleet of one that its run was "refused by every other node — `offload nodes` says which", about a
  machine whose fleet is itself.

- **…and then it said "nothing to hand over" about a run it had just stopped.** Third lie from
  the same command, and the one the last session left unconfirmed as a possible race. It is not:
  `left` came from `held_count()`, which answers a **capacity** question — how many runs is this
  node holding — and a checkpoint captured with `release = true` hands the run back to the pool,
  so it has *no holder by design*. So the one outcome worth reporting was the one outcome
  invisible. Measured: a run at turn 3, drained on a fleet whose only peer cannot host,
  checkpointed at turn 7, released, refused by everybody — and the CLI printed the words it
  prints on an idle laptop, while the daemon's own log said `nobody would take it; it stays here`.
  The same shape as `offload cancel` reading the process table: a question that *looks* like the
  right one, asked of the wrong thing at a different instant. Counted inside the pass now
  (`mesh::Drained`), because every run ends a drain in exactly one of three ways — handed over,
  mid-turn at the deadline, refused by every peer — and counting them as they go is exact.

- **…and the fifth lie was told by the fix for the fourth: one owner meant one *function*.**
  ADR-0034 moved `stop_accepting()` out of `Mesh::drain` and into `Request::Drain`, reasoning that
  "this node is leaving" is a fact about the node and that it should have one owner rather than a
  check at every call site — and then made it a call-site check at one of that function's **two**
  call sites. The other is the `SIGTERM` shutdown, which is how a laptop actually leaves: a service
  manager, a logout, `pkill offloadd`. Measured from the daemon's own log — `SIGTERM` at 15:55:41,
  `draining runs=1` a millisecond later, and `spawning claude code` at 15:55:44 for a run submitted
  three seconds into the shutdown, with `offload status` saying `accepting yes` and the operator
  told `run 01a043ef3018` at the keyboard. The node then killed that agent on its way out.
  `mesh::depart` is the third home for the flag and the only way in: it takes `Option<&Mesh>`,
  because a fleet of one is a departure too, and `Mesh::drain` is **private**, which is what makes
  two callers agreeing a compiler question rather than a thing to remember. The general shape: a
  fact hoisted out of a function into "the caller" has as many owners as that function has callers.

- **A drain waits minutes and, until it streamed, said nothing at all — including about the wait
  somebody could have ended.** `DEFAULT_ASK_PATIENCE` and `drain_deadline_secs` are both **300
  seconds**, and a run blocked mid-tool-call on `--ask` reaches no turn boundary until one of them
  expires, so `offload drain` was a blocking request in front of a five-minute pass: measured, forty
  seconds of blank terminal, while `offload asks` in the next window named the question and
  `offload status` said `waiting 1 run(s) stopped for an answer`. Two commands could say what the
  drain was waiting behind and the drain — the one somebody is standing in front of with their hand
  on the lid — could not. It streams now (`Response::Draining`, ADR-0035), and the step that matters
  carries the `tool_use_id`, because that is the difference between information and the way out:
  answered as the line said to, the drain returned **in the same second**. The report is not the
  fix's only half — the fix is that the wait is *avoidable*, and `Report::silent()` puts the same
  sentence in the log for the shutdown path, where the question is "why is this daemon taking five
  minutes to stop".

- **…and the same command reported the good outcome as the worst one.** `wait_for_checkpoint`
  answered `None` for **three** reasons — deadline reached, record gone, and the run *finished by
  itself*, which its own comment called "the nicest possible outcome" — and the caller read all
  three as `still mid-turn at the drain deadline` and counted them in `left`, whose whole meaning is
  "could not be handed over". So a run that had completed 150 ms earlier produced the sentence that
  keeps a lid open: *"either mid-turn at the deadline or refused by every other node"*. Measured
  twice in one walk, including on the shutdown path. `Boundary::{Reached, Finished, StillMidTurn,
  Gone}` and a third count on `Drained`; `Gone` stays in `left` with its own reason, because unknown
  is not good news. Same family as `offload logs`' single fallback for a `None` with two causes: the
  tell is a doc comment enumerating outcomes ("exactly one of three ways") beside a return type that
  cannot say which.

- **Two daemons on one state directory both start, and nothing looks wrong.** The second
  unlinks the first's socket and binds its own, so the first keeps its runs, keeps renewing its
  leases, and simply stops being reachable — one registry, two writers, no symptom. `statedir`
  locks the directory now, and `bind` *connects* before removing what looks like a leftover,
  because those are two different mistakes: the lock catches one `OFFLOAD_STATE_DIR` used twice,
  and the probe catches two directories configured to share one socket. The comment in `bind`
  asserted the lock for two phases before anything implemented it.

- **An agent outlives the daemon that spawned it, and fencing does not reach a process.** `kill
  -9` on `offloadd` four turns into a run, and the agent finished all twenty-one files of its task
  over the next minute — writing into a worktree the fleet had already moved the run out of, while
  the new holder ran the same run to completion. Both legs finished; the *record* was never in
  danger, and the work was still done twice. Nothing could stop it: `offload cancel` resolves a run
  to its **holder**, which is the other machine by then, and the machine the agent is on has no
  daemon listening; an epoch check refuses a *write to the record*, and a leftover agent makes no
  writes — it spends money and uses whatever grants the run was given. **And on one machine it is
  worse than wasted money**: `recover` marks the run `resumable with offload resume`, `resume`
  adopts the worktree *in place*, and the guard against two agents in one checkout reads the
  in-memory `live` map, which a just-started daemon has none of. Measured with the sweep disabled —
  two `claude` processes, one worktree, both working, reached by following the daemon's own
  instruction. It is the ordinary case too, because a daemon dying while the machine stays up is a
  crash or an upgrade, and for a run the *fleet* has moved on `recover` walks straight past —
  somebody else holds it, and recovering a peer's run is what that function must not do. The agent already had its own process group; what was missing was
  somewhere durable to write the number. `leftovers` is that, node-local (a pid means nothing on
  another machine), swept at startup **before** `recover`, and careful in three ways: the state
  directory lock is what makes "a note still here is stale" a guarantee, the note carries the
  session id because pids are reused, and a pid whose `/proc` entry cannot be read is **not**
  signalled — killing an unidentified pid is how a sweep for stray agents stops somebody's editor.

- **A run only fails if something says so.** `pump` ends when the agent's event stream closes,
  which normally happens *after* its `Result` event has moved the run to `Completed` or
  `Failed`. An agent that dies without a result — crashed, killed, a binary that is not an agent
  — produces no transition, and its holder is alive and renewing the lease, so nothing orphans
  it either: the run says `running` for ever and recovery cannot recover what was never marked
  failed. `fail_if_unfinished` is the answer, and the general shape is worth remembering — a
  state machine driven by *events* needs an answer for the stream simply stopping.

- **A node's own runs need tending whether or not there is a fleet.** Leases and failed-run
  recovery both live in `tend_own_runs` for that reason; anything about *this node's* runs put
  into the gossip tick silently does not run on a fleet of one, which is how phases 1 and 2 run
  and where an agent falling over at 02:00 still needs an answer.

- **Autonomy in failure follows attendance, never the deadline.** A run due in ten minutes with
  nobody watching should resume itself, because waiting for an absent human is how it misses the
  deadline; one due next week that somebody is watching should stop, because they can fix it and
  because retrying behind their back spends turns on the same broken tool call. An ordinal that
  bundled "soon" with "watched" got both backwards half the time. And **unknown is not
  unattended**: a daemon that restarted cannot see who was watching, so it leaves the run for a
  person — the other reading hands a crash-looping daemon every run on the machine, on every
  start, with nothing remembering it did.

- **A "fresh budget" that is a side effect of a deletion applies only where the deletion
  happens.** The retry count was cleared by the escalate branch removing its entry, so a person
  resuming a run that had been *given up on* got a fresh budget and a person resuming one still in
  backoff inherited its count. It is `stop_recovering` on the human path now, said once and
  deliberately not on the auto-resume path — which writes the count it is spending, so clearing it
  there is a spin loop with no memory of having happened.
- **A limit enforced only where it is reached is undone by whatever restarts runs.**
  `RunSpec::max_turns` was declared in phase 1 and read by nothing for seven phases (ADR-0039).
  Enforcing it at the turn boundary is the obvious half and is not sufficient: reaching the limit
  writes the run `Failed`, and `Failed` is the *input* `decide_recovery` turns back into a running
  agent — so a cap enforced only there is undone one backoff later, unattended, for as many
  resumes as the policy allows. `Escalation::TurnLimitReached` sits **above attendance** for
  `NotOursToRetry`'s reason: `Unattended` is the branch that resumes, and a run that quietly spent
  its budget overnight is unattended by construction, so a check below attendance is one the case
  it exists for walks straight past. The third point is `Supervisor::resume`, because a failed
  run's own sentence offers `offload resume` by name. All three read one predicate,
  `RunSpec::turn_limit_reached`, which returns the *limit* rather than a `bool` so the three
  sentences cannot disagree about the number.

- **A spent run must end, and specifically must not go back in the pool.** The tempting shape is
  the released checkpoint that already exists — capture at the boundary, hand the run back — and
  it is wrong in the one way that matters: `Pending` is what the next bidder picks up, so the run
  carries on past the limit on another machine, one leg at a time. Nor `Completed`, which is
  terminal *and* successful, so an abandoned run reads as a finished one; nor `Cancelled`, whose
  `by` names the node an operator typed `offload cancel` at, which here would be a lie in the only
  field that variant carries. `Failed` with the limit in the reason, and the capture taken first
  and unconditionally — whatever `every_turns` says, including `0` — because a capped run is
  precisely the one somebody wants to look at. Measured on a real Haiku run: `2/2` in `ps`,
  `failed: it reached its turn limit of 2`, `step1.txt` still in the worktree.

- **A refused request must not have the side effects of the successful one.** The resume handler
  called `stop_recovering` *before* `Supervisor::resume`, so all five of that function's refusals
  cleared the run's recovery entry as well — and `stop_recovering` removes it outright rather than
  resetting it, so the run fell out of the recovery bookkeeping permanently: no `recovery` line in
  `offload explain`, and the tick never revisited it because the tick iterates that map. Found by
  walking the turn limit, which is the one run that can *never* be resumed, so every attempt hit
  the refusal and erased the one place the reason was written down — `explain` said `it is over;
  nobody is streaming it` about a run whose own log named the limit. The same erasure was already
  reachable through "its agent is still shutting down here; try again in a moment", which asks the
  operator to repeat the request that does the damage. This is the entry above it — a fresh retry
  budget must not be a side effect — one caller further out, where the side effect belonged to a
  request that did nothing. **Not mechanically checked**: the handler wants a bound socket and a
  full `Ctx`, and `server.rs` has no harness for one; it was verified on a daemon, before and
  after.

- **A report that names a cause has to be told when it stops being the cause.** `explain`'s
  attendance line ended "— which is what decided", which was true for as long as attendance was
  the only thing that *could* decide a failed run's fate. `Escalation::TurnLimitReached` is the
  first reason that outranks it, and the line went on asserting the old cause: measured, `attended
  when it failed — which is what decided` printed one line above `left for a person: it reached
  its turn limit of 1`. Two answers to one question, in adjacent lines of one command. It now
  matches on the escalations that *are* about attendance rather than on the ones that are not, so
  a reason added later has to be listed here deliberately instead of inheriting a claim by being
  unlisted.
- **A request must survive a failed attempt to honour it.** `Supervisor::request_checkpoint` sets
  a flag and `offload checkpoint` replies "it will be taken at the next turn boundary". The pump
  read that flag with `mem::take` *before* attempting the capture, so a capture that failed
  consumed the request: the run went on to finish normally, never released, and nothing said so —
  the operator was promised an action and got neither it nor an error. Reachable before this by
  any capture failure, and it went from rare to routine the moment a transcript with no
  conversation in it started being refused, which is how it was found: the first walk of that
  guard showed turn 1 refused, turn 2 captured with `release=false`, and the run running to
  completion with the request gone. The flag is now read without clearing and cleared by the one
  arm that honours it — a request is a standing instruction until it is met — and it needs
  clearing nowhere else, because `release` drops the whole live entry when the leg ends. Walked
  after the fix: turn 1 refused, turn 2 `release=true`, run released, resume completed the task.
- **A drain's sixth lie, and the first one nothing in the command could have caught.** The entry
  above this one is its cause. `offload checkpoint` and `Mesh::drain` both call
  `Supervisor::request_checkpoint`, arming one flag, and that flag is now read *without* clearing —
  a standing instruction until honoured, which is right. What nobody looked at is that the drain's
  deadline stops waiting for it and does not withdraw it. So a run still mid-turn when
  `drain_deadline_secs` expires is reported as still here, and then, minutes later, finishes its
  turn, captures, and **releases itself** into the pool with no holder. Nothing offers it:
  `offload_core::supervise` returns `Bystanding::Unheld` for a checkpointed `Pending` run on the
  premise — written in its own comment — that such a run "was parked by a human with `offload
  checkpoint`", which is true of every caller but this one. Measured on two daemons and a fake
  agent with a thirty-second turn: the drain gave up at 10:40:59 saying `1 run(s) still here`, the
  run went `pending` at 10:41:19, and it sat there for as long as anybody watched — with `offload
  explain` reporting `checkpoint turn 1, copied to node-b` and `node-b bid 57, starting now` in
  the same output. Everything the migration needed was in place and one bid round away, and the
  round was never held. A drained laptop left holding a stopped run the whole fleet would have
  taken. `Mesh::owed` records what the drain could not finish and `Mesh::supervise` settles it at
  the boundary the drain could not wait for (ADR-0041); measured 3/3 afterwards, handing over
  nineteen seconds after the drain returned.
- **Only one of `depart`'s two callers can promise anything about later.** The fix above is a
  promise about the future, and only one caller can keep it: `offload drain` stops the node
  accepting and leaves it running, while a signal is the process going away — `main` stops every
  agent the drain could not move as soon as the pass returns. `Lifespan::{StaysUp, Exits}` is a
  parameter for this, and deliberately **not** a read of `Report::silent()`, which is the other
  thing that differs between those two callers and would have worked today. "Nobody is listening"
  and "this process is about to stop existing" are two facts that happen to coincide, and
  inferring one from the other is exactly how the departure flag ended up in the wrong place twice
  (ADR-0035 §3). Watched red on the shutdown path after the change: `still mid-turn at the drain
  deadline; leaving it to the lease`, `moved=0 left=1 finished=0`, agents stopped, and the peer
  orphaning and reassigning the run one second later.
- **This node is a holder like any other.** `fn owed(run, me)` decides what to do about a run the
  drain left owing, and its first version asked `run.holder().is_some()` for "has somebody else
  taken this". That answers *still ours, mid-turn* — the very state the debt is recorded in — and
  *somebody else took it* identically, while the two demand opposite actions: keep waiting, or
  stop and settle. The guard caught it on its first run, with `left: Done("somebody else holds it
  now"), right: NotYet` against a run this node was still running. The local id is now a
  parameter. Worth noting *why* the guard could catch it: the decision is a pure function of the
  record, so it is drivable with four `Run` values and no `Mesh`, `Cluster` or peers — the pass
  that acts on it needs all three, and a decision folded into that pass would have been checkable
  only by a walk.
- **The drain's seventh lie: a drained node starts agents.** The flag has been wrong in four
  places now, and this is the one that was never right rather than the one a fix broke.
  `supervisor.stop_accepting()` is checked at the **bid** (`Mesh::evaluate`) and at the **grant**
  (a grant can arrive after the bid that earned it), and `offload status` reports it. Auto-resume
  asks neither: `recover_failed_runs` walks this node's own recovery map on the ordinary tick, and
  nothing in `decide_recovery`'s seven inputs was about the node. So a departing machine went on
  starting agents for its own failed runs, unattended, indefinitely. Measured on two daemons with a
  fake agent that dies mid-turn: `offload drain` returned at 11:42:50 promising to hand the run
  over, the run failed at 11:43:23, and at 11:43:54 node-a logged `nobody was watching; resuming
  it resume=1` followed by `spawning claude code` — with `offload status` on that same node saying
  `accepting no — drained`, and `offload explain` showing `checkpoint turn 2, copied to node-b`
  while node-b sat idle. A laptop somebody closed, starting a fresh agent ninety seconds later.
  Note the first attempt to walk this proved nothing and looked like it had: the run died in turn
  1, before any boundary, so `decide_recovery` refused it for a *different* reason — "it has no
  checkpointed conversation to continue" — and the node's restraint was an accident of the
  fixture. The run has to reach a boundary first for this to be the question. `Circumstances.
  departing` is now checked above every other answer, because the rest ask whether a retry would
  work and this asks whether this is the machine to try it on; the guard drives the *most*
  resumable run there is — unattended, ours, in budget, past its backoff — and asserts the control
  and the departing case side by side. Left for a person rather than handed to the fleet: offering
  a **failed** run to peers is a decision nothing here has made, and the safe direction for a
  departing node is to start nothing and say so.
- **A start that gives up has to give the registration back — and the halt somebody put in it.**
  `Supervisor::start_run` calls `register` before anything that can fail, and it has to: the
  registration is what a concurrent `cancel_run` finds, and finding it is the whole of what that
  command does for a run mid-start. What installs the cleanup is `launch`, at the *end* — so every
  early return between the two leaves the entry behind, and there are two of them, a checkpoint
  with no session id and `ensure_blobs`. The second is a network fetch of a transcript, which is
  to say the ordinary way into this function for a migration. Nothing removes a `live` entry
  anywhere else in the crate: `grep` for it finds two hits and both are in tests. Measured with a
  guard, on the cheapest reachable version of it — a held run with a checkpoint and no fleet to
  ask: the row stayed `assigned`, the entry stayed in `live`, and `held_but_not_started` came back
  **empty**, which is the wedge. Two filters read that map and both are load-bearing:
  `held_but_not_started` skips a live run, so the tick never comes back to it, and
  `started_excluding` counts live-or-`Running` for a run this node holds, so the slot it is not
  using stays spent. A one-slot node meeting one unreachable transcript stops hosting for good.

  The half that is worse is the one the fix has to be careful about. `halt_agent` takes the cancel
  sender out of the entry, sends, and `cancel_run` answers **"its agent was stopped" while writing
  nothing at all** — deliberately, because the writer is `drive`, which owns the process and takes
  it down. If the start then fails, `drive` is never reached: hand the registration back and the
  halt goes with it, the row stays `assigned`, the operator has been told the run stopped, and the
  next tick starts it. `Supervisor::unregister` therefore removes the entry and takes the sender
  *in the same lock*, which leaves exactly two outcomes and no third — the sender is still here,
  so no cancel can ever be accepted, or somebody already has it, so their message is on its way
  and `recv().await` is bounded by their `send`. `Halt::Superseded` is deliberately not written
  down: that run is somebody else's and the record is theirs, and this leg giving up is what the
  halt asked for. The guard drives the whole thing through `start_held_runs` against a gated
  `Peers::fetch`, and goes red — `assigned`, not `cancelled` — with one line of the fix disabled.

- **The `live` entry outlives the run on purpose, and what it holds must not.** Three readers keep
  it alive after the agent is gone, and they are all right to. `release` drops only the cancel
  handle, because `drive` calls it *before* the terminal write so that a `cancel` or `resume`
  arriving in between finds no agent — and the run's last log lines are appended after it.
  `record_run` reads `cancel.is_some()` to tell a leg that is still driving from one that is not.
  And `describes` asks whether a finished run ended **here**, which for a run with no holder is
  answerable only from this leg's epoch, which is in that entry and nowhere else. So the entry has
  to stay.

  What did not have to stay is the channel. `broadcast::channel(256)` allocates its ring when it is
  created, and nothing in the crate removed a `live` entry — `grep` found two hits and both were
  tests. Measured in-process, with the store deliberately kept out of it (events sent straight to
  the sender rather than through `append`) and each case in its own process because allocator reuse
  contaminates a shared one: **37,806 bytes per run**, identical whether or not anybody ever
  followed it, against a control that removed the entry and grew by nothing it kept. Then measured
  on the product, which is the number that matters: a daemon submitting 100 one-turn runs at a fake
  agent grew its RSS by **9,836 KB and 9,868 KB** on two passes before, and **7,420 KB and 7,060 KB**
  after — about **26 KB a run** reclaimed, reproducibly. The rest of the ~72 KB a run that remains
  is the store and its page cache, which is a different question.

  Why it matters at all: ADR-0020. A rule firing every three seconds is 1,200 runs an hour, session
  thirty-three measured exactly that cadence leaving eight worktrees in forty-five seconds, and
  this daemon is meant to run on a phone. 44 MB an hour of rings for runs that finished is not a
  rounding error.

  `close_stream` sets the field to `None` at the end of `launch` rather than inside `drive`,
  because it has to be after every way the leg can end — including the arm `drive` returns an error
  to, where `fail` writes the run's last line. A follower still attached drains what is buffered
  and then sees the channel close, which is what `offload logs -f` is waiting for. Two operator-
  facing lines got *more* accurate as a side effect, both in `attendance_now`: a checkpointed run
  released back to `Pending` said "unattended — nobody is streaming it" and now says "not running
  here", and a superseded run held on another node said the same thing and now says "only <node>
  can tell; it is running there".

  `forget_legs` is the other half: a leg is a fact about a run, so it has no business outliving the
  run's **row**, and `prune_spent_records` — ADR-0020's own pass, on exactly the runs a node has
  thousands of — deleted rows and dropped the list of what it deleted on the floor. It returns them
  now. Safe by construction rather than by care: `describes` and `holds` both start from
  `self.run(run_id)`, which is `None` for a row that is gone, so there is no answer this can change.
  `cancel_all` was tightened in the same pass, for the same reason read backwards — it iterated
  every key in the map, which is every run the daemon has ever started, awaiting a `halt_agent`
  that answers `false` for all but a few; it now filters on the handle, which makes the shutdown
  pass proportional to what is running rather than to what has run.

- **"An entry exists" and "an agent is going here" are not one question, and `live` has never
  answered both.** Session thirty-eight established that the entry outlives the leg on purpose and
  wrote down which of its parts must not. What it did not ask is what the *readers* were doing with
  the part that stays. Five of them spelled "has this run started here" as
  `live.contains_key(&run.id)`, which was the same thing only because nothing ever left the map
  while the daemon lived.

  There are three facts in one entry and they end at three different moments. `cancel` is *an agent
  is going here and can be halted*, dropped by `release` one step before the terminal write.
  `live` is *this leg may still append*, dropped by `close_stream` at the end of `launch`, after
  the last line. `epoch` is *this node ran a leg, numbered E*, and it stays until the run's row goes
  (`forget_legs`) because `describes` asks whether a finished run ended **here** and for a run with
  no holder nothing else can answer. Its owner is this node and it is gossiped nowhere, so ADR-0005's
  arbitration question does not arise; losing it early is safe, and losing it is *normal*, since a
  restart empties the map and every reader of the epoch already has the restart's answer.

  The door in is `hold_here`, which does not touch `live`. A run granted back to a node that had
  already run a leg of it — laptop, desktop, laptop — lands beside the previous leg's entry, and
  `awaits_a_slot` reads that entry as *already started*. The run stays `Assigned`, renewing a
  perfectly good lease, with no agent behind it and no tick that will ever look at it again.
  Nothing fails and nothing says anything.

  Walked on two daemons, same sequence both sides, one line apart. With `agent_here`: granted to a
  full node-a at 21:19:23, both slots freed at 21:20:59, agent running at 21:21:11 — resumed at turn
  17 from the checkpoint it left at 14. Reverted to `contains_key`: granted at 21:31:24, slots freed
  at 21:32:53, and at 21:36:25 still `assigned` with a fresh lease on a node reporting
  `runs 1/2 · accepting yes`. The 1/2 is the wedged run itself, so it holds the slot it will never
  use. Staging it needs a leg on A, a release, a second leg elsewhere, and a grant back while A is
  full — `offload checkpoint` on A, `offload resume` from B, then `kill -9` B and fill A before its
  orphan timer expires.

  The same misreading made `started_excluding` count such a run against the machine's capacity —
  which is the deadlock `two_commitments_on_a_one_slot_node_do_not_block_each_other` exists to
  prevent, arriving through a different door — and made `release_unstarted` leave the commitment
  behind on a drain instead of handing it back. The log page's "is it still live here" got
  `stream_open` (`live.is_some()`) rather than `agent_here`, because that question ends one step
  later: between `release` and `close_stream` the run's last lines are still being appended, and
  asking for the agent would end a follower early.

  Why it shipped: every test that touches `live` registers a leg and leaves it registered, so the
  two predicates agreed in all of them. The one case where they differ needs a *second* leg for the
  same run on the same node with no restart in between. Session thirty-seven's fix — hand the
  registration back when a start gives up — is this same bug met from the only side anything had
  walked, which is why it read as being about failed starts rather than about the predicate.

- **A refusal *now* is not an answer about later, and a drain released the run before it asked.**
  ADR-0041 decided that a drain's deadline bounds how long somebody waits, not what the node
  intends, and gave a run still mid-turn at the deadline a debt: `Mesh::owe`, retried on the
  backoff until somebody takes it. A run that reached its boundary **inside** the deadline was
  offered exactly once, and a refusal at that instant was read as final — so the run that behaved
  better got the worse outcome, and a fleet that happened to be busy for ten seconds cost it its
  whole life.

  "Final" understates it, because the drain releases before it offers. A refused handover leaves
  the run `Pending`, with a checkpoint and no holder — which is precisely what `offload checkpoint`
  leaves, and `offload_core::supervise` reads it as parked by a person (ADR-0014) and passes by.
  Nothing else offers it. This is session thirty-four's own shape: one piece of state written by
  two callers with different intentions, where the reader cannot tell them apart.

  Measured on two daemons with the peer down: `offload drain` at 21:47:32 said `nothing could be
  handed over; 1 run(s) still here`, and `offload explain` then described the run as *"parked with
  its checkpoint, and waits for `offload resume` rather than for a bid round"* — the sentence for a
  run a person parked, about one nobody had. Both daemons restarted, idle, accepting, checkpoint on
  disk: still `pending` for as long as anybody watched. On the fix: drain at 21:56:53 refused,
  node-b started 21:57:02, handed over 21:57:25 at turn 6 in the same conversation.

  `unplaced` is the decision, pure and beside `owed` for that one's reason — what a pass holding a
  `Mesh`, a `Cluster` and a peer decides inside a branch is a decision nothing can check. It turns
  on the one thing that can stop it: `main` stops every agent the drain could not move as soon as
  it returns, so `Lifespan::Exits` has no later to promise and strands the run, which is a named
  residual rather than an oversight. The debt is in memory, so it does not survive a restart
  either — the second case now wanting the durable field the handoff has twice deferred.

  Two reports moved with it. `Drained::later` counts both kinds of debt, so the CLI stops telling
  an operator that a run the fleet refused is "still mid-turn". And the warning said "nobody would
  take it; it stays here", which was the load-bearing falsehood — it does not stay here, it goes
  back to the pool.

  Why it shipped: every test of the refusal branch asserted the *count*, and `left` was the right
  count for the outcome as it then was. Nothing asserted what became of the run afterwards, and the
  only way to see that is to look a minute later at a fleet that has since had room.

- **And a fifth path, for a reason one step stronger than the drain's.** The four the drain flag
  had been missing from are above; the fifth belongs to a **revoked** node, and its full entry is
  in `membership-and-credentials.md` because that is the subject it was found in. What belongs
  here is the shape, which is this series' exactly: auto-resume asks neither the bid nor the
  grant, so ADR-0044 shutting those two doors would have left the one that spawns agents open —
  on a machine the fleet had thrown out and whose runs it had already taken back.

  Two things are different from the drain's case, and both are the reason it is not simply another
  `departing`. It is checked **above** `departing`, because the questions are different in kind: a
  drained node is the wrong *place* to start an agent, a revoked one is not permitted to be a
  place at all. And it does **not** wrap `Resume`/`Wait` into `Recovery::LetGo` the way ADR-0043's
  does — letting go is an offer to the fleet, and a revoked node has no fleet to offer anything
  to: nothing it writes gossips, no checkpoint of its replicates, and the run is already the
  fleet's under a higher epoch. `Escalation::NodeIsRevoked`, left for a person, swept by the same
  shape of property test the departing one has.

- **A `bool` that arrives among small integers is a call site nobody can read.** Adding
  `departing` took `decide_recovery` to eight positional arguments — `(&run, attendance, standing,
  false, 0, 0, now, &policy)` — and clippy's `too_many_arguments` was the useful accident that
  stopped it. There is precedent for `#[allow]` in this workspace (two node-side functions), and
  taking it here would have been the wrong trade: this is the pure decision function the
  simulation tests drive, and a bare `false` between two counts is a miswrite waiting to happen —
  one nearly went in while writing the guard for it. `Circumstances` names all five observations at
  every call site. What that buys beyond readability is the pair that must agree: the recovery tick
  and `offload explain` ask this same question, and `Observed::turns`' doc already says why they
  must come out of one function — with eight positionals, keeping them in step was lining up two
  argument lists by eye.

## …and the answer was a field on the run, which retired the debt that found it

The entry above put the drain's unfinished business in `Mesh::owed` — node-local, in memory,
beside the departure flag — and said in as many words what it was leaving: the debt does not
survive a restart, and should not, because a daemon that restarts has departed for real. That is
true of the *departure* and false of the *run*, and the gap it left had three doors rather than
the one narrow window ADR-0041 described.

1. `offload drain`, then the daemon stopped before the fleet has room.
2. `Lifespan::Exits` — a `SIGTERM`, which is what closing a laptop looks like. The run reaches its
   boundary inside the deadline, releases, is refused, and nothing records that anybody owed it
   anything: `unplaced(Exits)` was `Strand` by construction, because a process on its way out
   cannot promise a later.
3. ADR-0041's own residual, which is (1) said from the other end.

Measured on the prior build, through door 2, one node and a fake agent:

```
07:07:14  kill -TERM        →  draining runs=1
07:07:15                       nobody would take it right now   refusals=1
07:07:17  the daemon exits
07:07:17  restarted — idle, accepting, holding the checkpoint at turn 4
07:08:32  state    pending — waiting for a node since 1m17s ago
                   nobody holds it: it is parked with its checkpoint, and waits
                   for `offload resume` rather than for a bid round
```

and on the fix, the same sequence: released 07:05:11, exited 07:05:13, restarted 07:05:18, `an
unheld run was placed offering=LetGo { by: node-a }` at 07:05:19, resumed at turn 3. Through door
1, with the decision reverted and restored on the same daemon: reverted, released 07:03:36,
restarted 07:03:45, still pending at 07:04:35; fixed, released 07:02:36, restarted 07:02:52,
running at turn 5 at 07:02:53.

`RunState::Pending { since, let_go_by: Option<NodeId> }` is the fix (ADR-0042), and the reasoning
about *where* it lives is in `gossip-and-merge`. Three things about it are lifecycle rules:

**The reason travels with the request, because the release cannot work it out.** `offload
checkpoint` and a drain arm the same flag and produce the same capture at the same boundary; the
only moment that knows which is which is `Supervisor::request_checkpoint`, minutes and a turn
earlier. `GivenUp::{Parked, LetGo}` rides from there through `LiveRun::checkpoint_requested` —
which was a `bool` and is now the answer — into `Run::checkpointed`.

**`LetGo` wins when both arrive.** A drain waits minutes and a person can type `offload
checkpoint` inside that window; the node is leaving whatever else was typed at it, and writing
`Parked` on a run nobody will come back for is the stranding this field exists to stop. Making
that reachable meant `Run::request_checkpoint` becoming idempotent from `Checkpointing`: it used
to refuse a second request, which was harmless while a request was only "somebody wants a
checkpoint" and stopped being harmless the moment it carried who — a drain reaching a run a
person had already parked was turned away, and the run released as parked on a machine whose
owner had just said it was leaving.

**`Mesh::owed` had to be deleted, not merely left unused.** With the fact on the record, the run's
*arbiter* offers it; with the debt still in place, the departing node offers it too. Those are
two nodes running a bid round for one run, which `place`'s own comment calls the case the epoch
cannot settle — two arbiters both bump `next()` from the same number, so the grants cannot be
ordered and `merge_run`'s equal-epoch tiebreak is left holding a decision it exists only to back
up. Nothing was lost with it: a draining node's own `evaluate` answers `draining`, which is
exactly the bid `finish_owed_handovers` passed by hand, and both offers were already rate-limited
by the same `may_retry` backoff.

## …and a failed run had the same shape, one state further on

ADR-0034 found the fourth path `stop_accepting` had never reached — auto-resume spawning agents
on a drained laptop — and closed it by making `departing` the first question `decide_recovery`
asks and `Escalation::NodeIsDeparting` its whole answer. Right about the agent, and it ended the
question one step early.

**"This node will not start an agent" and "nobody should" are two facts.** A drain is the one
moment they come apart. The run is resumable, unattended, ours to retry, inside its retry budget
and past its backoff — every other answer in that function says *resume* — and the only objection
is a machine that is going away. ADR-0034's own doc comment stated the limit and its reason:
"handing a *failed* run to the fleet is a decision nothing here has made, and the safe direction
for a departing node is to start nothing and say so."

**And "left for a person" was not what happened to it.** `offload_core::supervise` returns
`Bystander(Terminal)` for a failed run, so no arbiter offers it and no node bids on it; the
recovery entry holding the reason is in memory and dies with the daemon. Measured on two daemons,
one fleet, a fake agent failing after two turns with its checkpoint replicated to an idle peer:

```
08:49:52  offload drain    →  "nothing to hand over"
08:50:23  offload explain  →  state    failed — failed 31.1s ago
                              recovery left for a person: this node is draining, so it
                                       will not start an agent for it
                              checkpoint turn 2, copied to fedora
08:51:24  node-b: 01a0516e4b1f  failed  2 turns  replicated
08:51:50  both daemons restarted →  still failed, and now with no reason on screen at all
```

Three things there are worth reading together: the drain says it did nothing about a run it had
just decided about, the run's explanation names the drain as the cause of something the drain
cannot fix, and a peer holding a copy of the conversation is idle throughout.

ADR-0043 is the fix, and four things about it are lifecycle rules.

**`departing` wraps the run's verdict instead of replacing it.** `Resume` and `Wait` become
`Recovery::LetGo`; every other answer passes through unchanged, because it is about the run and
is therefore just as true of the next machine — a run that spent its turn limit spent it
everywhere, and one somebody was watching is theirs wherever it goes. `Wait` is included on
purpose: a backoff is a promise to try again in thirty seconds and a departing node has none to
promise, which is ADR-0041's rule (a refusal now is not an answer about later) said about a wait.
`NodeIsDeparting` retires with the change, because it named the node when the question was about
the run — a capped run was told the drain was what stopped it, which sends somebody to restart a
daemon.

**The guarantee ADR-0034 bought is now structural rather than a line order.** `Resume` and `Wait`
are the only answers that spawn an agent, there is one function that can produce them, and the
departing match is the one place that decides a leaving node may not hear them. A property test
sweeps every shape of failed run — pinned, capped, unreplicated, checkpointless — against every
`Circumstances` a departing node can present, and asserts neither ever comes back. The old check
was a comment claiming the test was above the others.

**The effect is a transition, not a start.** `Run::let_go`: `Failed` → `Pending { let_go_by }`,
epoch bumped, unfenced for `reopen`'s reason (a failed run holds no lease) and inside
`Store::update_run` for the fence's. This is what makes the ADR small — everything downstream is
ADR-0042's and untouched. It is also the answer to the question ADR-0042 left open, which is that
the question dissolves: that ADR expected a failed run to need its own home for the fact, since
`let_go_by` lives inside `Pending` and a failed run is not pending. It does not, because the run
*stops being failed*. Ask what state the fact wants to be true in before inventing a place for it.

**Only where the fleet could actually start it.** `Checkpoint::is_durable(run.holder())` — `None`
for a failed run, so it asks the plain question — else `Escalation::NoCopyElsewhere`. A run
offered without its conversation is one the winner cannot begin: it takes the run, fetches
nothing, and leaves it `Pending` behind a checkpoint that may never be reachable again, which is
**worse than the `Failed` it came from**, because `Failed` carries its own reason.

## A departing node's own pass is not the recovery tick's, and the record has to leave with the goodbye

Both halves were found by walking the fix, one after the other, and neither is optional. They are
the same mistake at two layers: assuming something that normally happens a moment later will
happen at all, on the one path where there is no later.

**The sweep belongs to `depart`.** `Mesh::drain` filters `!run.state.is_terminal()` into `held`,
so it has never seen a failed run — and a node whose only run has failed holds nothing, which
makes that pass return on its first line. The recovery tick would get there within a second on
the `offload drain` door, where the daemon stays up. On the signal door there is no next tick:
`main` sends the shutdown and stops everything the moment `depart` returns. That is the `SIGTERM`
door, which is what closing a laptop looks like, and the reason the pass goes in `depart` is the
one the departure flag has already taught twice — it is the only way in, and a fact hoisted into
"the caller" has as many owners as that function has callers.

**And the record has to be in the view before the process dies.** Measured on a build with the
sweep and without this: node-a logged `handing the failed run back to the fleet`, wrote `Pending`
to its own store, exited — and node-b went on holding the run as `Failed`, indefinitely, until
that daemon came back. The stranding this whole decision is about, moved from the record to the
network. `Cluster::publish_run` writes into the local view only; what leaves is the periodic
republish, a tick away, or `announce_departure`, which carries `Gossip::runs` and is already
waited for one step after `depart` returns. Publishing into the view is therefore the whole fix,
and it is why the departing node — not the arbiter, not the peer — is the one that has to say it:

```
08:47:26.265  node-a  handing the failed run back to the fleet
08:47:26.268  node-b  peer is leaving          ← the goodbye, carrying the record
08:47:26.510  node-b  an unheld run was placed  offering=LetGo { by: node-a }
08:47:26.530  node-b  spawning claude code, same session
08:47:46      node-b  completed, 4 turns — having failed on node-a at turn 2
```

265 milliseconds from the signal to the run being placed on another machine.

**`Capacity::runs(0)` on the sweep is a guard, not a placeholder.** Capacity is read in exactly
one place in that pass — the resume — which a departing node cannot reach. Passing nought rather
than the node's real figure makes the unreachability a second and independent guard: if the first
ever breaks, the resume is refused for want of room instead of starting an agent on a machine
that is leaving.

## A function that asks "could anybody *else* do this" has to check that anybody else exists

`fleet_could_start` decides whether a node hands a failed run to the fleet or keeps it, and its
own name is the question. For a `Resumable` run it asked `is_durable` — does another node hold a
copy of the conversation. For an `Idempotent` one it answered **yes unconditionally**, on the
reasoning that such work is re-runnable from its spec and there is nothing to fetch. True, and
half the question: somebody has to be there to re-run it.

Measured on one daemon with `cluster.enabled = false`: two tasks that exit 2, a `SIGTERM`, a
restart.

```
run 01a08b41…   pending   epoch 2   last assigned to walker
run 01a08b42…   pending   epoch 2   last assigned to walker
```

Released into a pool with no members. Nothing ever offered them again, *including the node that
came back a second later*, because offering is the cluster's job and there was no cluster.
`offload explain` was exactly right about both halves — *"this node offers it to the fleet until
somebody takes it"* and *"nobody was asked to take it: this node is not in a mesh"* — so nothing
was hidden. What was wrong is that nobody ever would, and because a **task cannot be resumed**,
`offload resume` was not even the way out.

The second consequence is the one that bit. `rule_run_in_flight` reads *non-terminal* as
in-flight, so a stranded occurrence **jams its rule for ever**: `offload rules` reported `running
now 01a08b42…` about a run nothing would ever run, and every later firing was dropped — silently,
on a plane whose whole job is to be loud. On an escalation rule (ADR-0057) that is a recovery
plane that stops working and says nothing.

**`Circumstances::alone` is the missing input**, and it is deliberately *any node this one has
met* rather than any node answering right now: a peer in a bag comes back, and a `Pending` run
waiting for it is exactly what ADR-0043 wants. What it rules out is the fleet that has never had a
second member. `ClusterView::alone` is the single definition, asked by the recovery tick, by a
departure and by `offload explain` — a fleet of one that two of those disagreed about would be
worse than the bug.

**The escalation is its own variant**, not a reuse of `NoCopyElsewhere`: that one is about the
**run** — its conversation is on one disk — and this is about the **fleet**. Telling somebody with
one laptop that their *task's* conversation exists nowhere else is a true sentence about a thing
that has no conversation, and a confident wrong answer about why nothing happened.
`Handover::{Possible, NoCopy, NobodyElse}` carries the three answers out of the predicate, because
two negatives collapsed into a `bool` is how a report comes to name the wrong cause.

After: three failed tasks stay `Failed` across a drain and a restart, nothing is handed back, and
the rule reads `last fired 01a08b4268b4` rather than `running now`. The two-node handover is
unchanged and ADR-0054's test still passes with `alone: false`.

## …and the fix is not only in the drain, which is the part that needed looking up

`LetGo` is reachable without departing: a node whose owner will not let it host (ADR-0049)
releases a failed run the same way, so a laptop under its battery floor on a fleet of one strands
runs with the daemon still running. `tend_own_runs` therefore takes the cluster for **one** fact,
and its doc comment says which: the fleet's *size*, never its opinion, which is what keeps that
pass out of the gossip loop. It is spawned after the mesh now; `mesh::start` awaits no dial, so
the move costs nothing.

That second cause needs its own **sentence**, which makes this a 2×2 and not a pair. Leaving or
refusing, crossed with no copy of the conversation or no second device, is four answers — and the
first version of this fix had three. The owner-refused corner borrowed `OwnerWillNotHost` (*"no
other device holds a copy"*) for an idempotent task that **has no copy to hold**, which is the
mistake `NodeIsDeparting` was deleted for (ADR-0034 §1) arriving from the other axis.
`OwnerWillNotHostAndAlone` is the fourth, and the distinction is what the owner does next: plug
this one in, or add a second device. Both pairs are computed in one `match`, so a new cause cannot
pick up an old cause's words.

**And that corner cannot be walked on a single machine**, which is a property of the gate rather
than a gap in the walk. `permits` refuses on `accept`, charging, the battery floor, a metered link
and `allowed_agents`. The first and last are **config**, read once at startup; the middle three
come off the probe. So the hosting gate can only disagree with the placement gate that already let
the run start if a *capability changed underneath it* — unplugged, or tethered — which is exactly
the premise of ADR-0048 and ADR-0049, and is not scriptable here without root.

What was attempted, and what it actually showed: `accept = "always"`, a task that exits 2, then a
restart under `accept = "never"`. The run stayed `failed` at epoch 1 — but **not** because of this
fix. `recover_failed_runs` iterates `self.recovery`, an in-memory map a new process starts empty,
so the run was never considered at all; the daemon logged no decision and `offload explain`
reported none, which is the same honesty as `Escalation::AttendanceUnknown` (a node that has
forgotten must not answer as though it remembers). Two things worth carrying from that: a restart
is **not** the cheap way to re-drive this pass, and the danger was always the **drain**, which
runs the pass with the map still populated and `departing` set. The enforcement for the fourth
corner is therefore the unit test, said plainly here so the next session does not read the walk as
covering it.

- **The fourth agent-only path, and the drain that waited for it.**

  Three paths in `Supervisor` ask for the agent half of a `Work` and refuse a task with a reason —
  `prepare_start`, `resume` and `drive` — and `Work::Task`'s own doc comment names all three. There
  is a fourth. `Supervisor::request_checkpoint` guards on:

  ```rust
  live.get_mut(&run_id).filter(|l| l.cancel.is_some())
  ```

  which asks *is something running here*, and `start_run` takes the cancel channel at
  `self.register(run_id, epoch)?` — **one branch above** the `match` on `Work::Task`. So a task
  passes a guard written for an agent.

  Walked on a daemon with `[[tasks]] command = "/bin/sleep", args = ["90"]`:

  ```
  $ offload checkpoint 01a0925e0605
  checkpoint requested — it will be taken at the next turn boundary
  watch for it with: offload logs -f 01a0925e0605

  $ offload ps
  RUN            STATE          KIND   …
  01a0925e0605   checkpointing  task

  $ offload explain 01a0925e0605
  state       checkpointing — finishing its turn before checkpointing, asked 10.2s ago
  ```

  *Finishing its turn*, about a `/bin/sleep`. The run then ran to completion and nothing was ever
  captured; `offload logs -f`, which the command had pointed at, had nothing to show.

  **The costly half is the drain.** `Mesh::drain` arms the same flag for every held started run and
  then waits per run, up to `drain_deadline_secs` — **300 seconds by default**. Measured with the
  deadline cut to 20s and one task running:

  ```
    waiting for 1 run(s) to reach a turn boundary — up to 20.0s
    1 run(s) still mid-turn — this node hands each over at its next
    turn boundary, without being drained again.
  ELAPSED: 20s
  ```

  Twenty seconds spent on a boundary that cannot arrive, and then a promise about the next one.
  After the partition, with the same staging:

  ```
    1 task(s) still running here — a task has no turn boundary, so
    nothing waits for one. Leave the daemon up and it finishes; stop it and
    the fleet runs it again from its spec.
  ELAPSED: 0s
  ```

  and the mixed case — one agent run and one task held together — waits for the agent alone
  (`waiting for 1 run(s)`, 20s) while the task gets its own line.

  Three things about the shape:

  - **The guard is in `offload-core`,** on `Run::request_checkpoint`, because the run knows its own
    kind and two callers guarding is two things to remember. That is `Supervisor::register`'s
    argument for holding the second-agent check, one layer out.
  - **`TransitionError::NoBoundary`, not `WrongState`.** The state was `Running` — exactly when a
    checkpoint request is legal — so a sentence naming the state would be true and would send
    somebody to wait for a moment that is never coming. What is wrong is the kind of work.
  - **The fleet-of-one arm counts from `held_count()`**, not from the pass that partitions, so it
    needed `Supervisor::held_no_boundary` as well — otherwise a running task is `no_boundary` on
    one path and `left` on the other, which is one fact with two spellings and the exact trap
    `finished`, `later` and `pooled` were each split out of `left` to avoid. Its `left` sentence,
    *"there is no fleet to hand it to, so it stays here with its checkpoint"*, is false for a task
    twice over: it has none, and it never will.

  The reusable question is not the one the previous seven drain entries answer. Those were *how
  many callers* and *how many doors*. This one is **what kind of work is it holding** — a question
  that became askable the day ADR-0019 added a second tier, and that nothing in the drain had ever
  asked. It was found by walking `offload checkpoint`, which nothing had walked, against the
  cheapest thing to point it at.

*The seven entries below were backfilled in session ninety-one from `docs/sessions.md` and the
commits that added each rule (`613fea0`, `c0248c4`, `a215adb`, `644fc40`, `73ce8e3`) — sourced, not
reconstructed from the rule text.*

- **…and a sixth: the one a person types.**

  Session forty-seven (commit `613fea0`, ADR-0047). ADR-0046 stated its invariant — *`permits` is
  asked on every path that can start a run* — about one door. `offload resume` starts an agent the
  same way a submission does, and its handler asked none of the three questions: it resolved the
  id, built a `Room` and called `Supervisor::resume`, so the only gate was the queue. Measured on one
  daemon, three times:

  ```
  accepting   no — node is not accepting work                       → resumed, turn 6
  accepting   no — drained; restart offloadd to take work again     → resumed, turn 5
  accepting   no — this node has been revoked from its fleet …      → resumed, turn 7
  ```

  The third undoes ADR-0044: a node revoked a minute earlier, which had halted its agent, written
  the revocation onto the run and refused `offload run` with the fleet's own sentence, resumed the
  agent when asked by name.

- **…and a seventh, which is the one nobody types at.**

  Session forty-nine (commit `c0248c4`, ADR-0049). Measured with session forty-eight's fake `nmcli`
  and a fake agent that fails on its first leg:

  ```
  16:21:45  run starts, link free
  16:22:06  what this device is has changed          (metered)
  16:22:33  run failed; noted who was watching       attendance=unattended
  16:23:06  nobody was watching; resuming it         resume=1
  16:23:06  spawning claude code
  ```

  For that whole minute the node reported `accepting no — network is metered and policy disallows
  it`, refused a submission at its own socket, and refused an `offload resume` of that very run —
  then started the agent itself, from the recovery tick. `Circumstances::hosting` now makes the tick
  behave like `departing`: `Resume` and `Wait` become `LetGo` under ADR-0043, with its own sentence
  (`OwnerWillNotHost`) where nobody else holds a copy.

- **…and a sentence has as many places as it has callers.**

  Same session (commit `a215adb`). The `LetGo` half, walked on two daemons once an unrelated process
  stopped pinning the load average: 412 ms from decision to handover, turn 12 to turn 16 on bravo,
  alpha never starting an agent. On the first pass the log line read *"this node is leaving; handing
  the failed run back to the fleet"* — about a laptop sitting right there on a metered link. The ADR
  had fixed the two places somebody would look (the escalation and `offload explain`); the
  `tracing::info!` a few lines from the transition was the third, found by running it:

  ```
  16:48:49.536  handing the failed run back to the fleet
                  reason="this node's owner does not allow it to host runs"
  ```

- **A gate has as many doors as there are ways to start an agent, and the last one found is the one
  that opens by itself.**

  The summary of sessions forty-seven to forty-nine. Forty-seven: count the doors. Forty-eight: ask
  how fresh the fact behind them is. Forty-nine: the last door to be found opens without being
  asked. Seven doors on this gate by then — the bid, the grant, the no-cluster arm's drain clause,
  the tick's `departing`, the tick's `revoked`, the policy on both doors a person types at, and the
  tick's own. Every fix was measured, correct and written up; what kept being missed was the
  counting.

- **…and the eighth thing about that gate is not a door, it is which cause it names.**

  Session fifty (commit `644fc40`). Pairing `offload ps`'s new resume footnote with each door, on a
  revoked fleet-of-one the footnote read `not of this node right now: this node is draining`, one
  line under a run that had failed with `this node was revoked from its fleet`. `stand_down` sets
  **both** latches (ADR-0044) and `hosting_refusal` asked the drain first, so every door it guards
  told an evicted device's owner to restart `offloadd` — which changes nothing; the way back is
  `offload join`. `status` had the right ordering, with its reasoning written beside it, the whole
  time. Measured at the resume door against the submit door a second later, which reaches the fleet
  state through the mesh and named the revocation correctly.

- **One decision, two mechanisms, and only one of them existed: a failed task was never
  restarted.**

  Session sixty-eight, phase 8's demo (commit `73ce8e3`, ADR-0058). `decide_recovery` answered
  `Recovery::Resume` for an unattended failed task, and the arm acting on it called
  `Supervisor::resume`, which refuses a task on its fourth line for want of a conversation. The tick
  asked every thirty seconds, was refused in the same millisecond, **counted the refusal as a spent
  attempt**, and the program never ran again — while `explain` said *"picking it up again in 16.7s
  (try 1)"*. `Supervisor::restart_task` is the task tier's door now, dispatched on `Work::kind()`;
  `offload resume` still refuses a task, because that is the door a person types at.

- **…and the fix that suggests itself contradicted a decision the tree already held.**

  Same session. The first fix was an escalation — *a program that already ran is not run again by
  itself*, on the real argument that a task exiting 2 may have sent half its mail. Four tests failed
  on the first run: ADR-0043 has a **departing** node hand a failed task to the fleet precisely so
  another node re-runs it from its spec, walked one session earlier. Before deciding a mechanism is
  missing, find the place that already decided it was not. The argument's home is a restartability
  field on `TaskConfig` (ADR-0058's residual), unbuilt because nobody has asked.
- **A stop that returns at the signal is not a stop, when a start can follow it.** Found while
  writing the Swift caller for ADR-0070, before anything ran. `applicationWillEnterForeground` calls
  `offload_start`, and `offload_stop` had taken the sender out of the `STOP` slot, sent it and
  returned. The daemon was still draining on its thread. An app sent to the background and brought
  back inside that window got `0` from start, and a second `daemon::run` on the same state
  directory, lock and socket. The slot holds `Running { stop, thread }` now, and stop joins the
  thread. A start that finds a *finished* thread in the slot (a daemon that ended with an error)
  replaces it rather than answering "already running" for ever. The test asserts the socket refuses
  a connection the moment stop returns. Control-run with the join commented out: `stop returned
  before the daemon had gone`.
- **An absolute path inside an iOS app's container goes stale on every reinstall and update.**
  Walked. After `simctl install` of a rebuilt app, `nodes` on the iOS node said "not in a mesh". The
  data container had moved, from `…/Application/E33292ED…` to `…/E9B80131…`, with `fleet.json` and
  `node-key` carried across. `node.toml` still said `state_dir = "…/E33292ED…/Library/s"`. The
  daemon created that path, found no key, and minted one: a new node id outside the fleet. The
  Simulator does not enforce the sandbox; a device would have refused the path and the daemon would
  not have started. `Paths.repoint()` rewrites the top-level `state_dir` (and `socket` on a device)
  on every launch. After it, a second reinstall moved the container to `…/B7946337…`, and the node
  came up as `56af…`, a member, and meshed.
- **A host app at targetSdk 35 draws edge to edge whether or not the layout expects it.** Walked
  on the Samsung tablet (Android 16). `uiautomator dump` gave the "Key" button's bounds as
  `[531,0][663,72]`. A tap at (597,36) did nothing, and a screenshot showed the app's title bar
  where the buttons should have been, with the status text starting under it. Android 15 enforces
  edge to edge for apps targeting 35, so the content view begins at y=0 behind the system bars.
  Fixed with `Theme.DeviceDefault.NoActionBar` on the activity and an
  `OnApplyWindowInsetsListener` that pads by `WindowInsets.Type.systemBars()`. Afterwards the row
  showed at y 45–104, and the tap made the key.
- **Anything a host app calls on the platform's behalf can throw.** The first prompt crashed the
  app. `logcat -b crash` showed `SecurityException: Must have USE_BIOMETRIC permission`, from
  `ApprovalKey.answer` called in `MainActivity.onResume`. The waiting `offload invite` would have
  timed out after 120 s with "is the app open?". Fixed by declaring the permission and wrapping
  `authenticate` so a failure writes `<id>.refused`. The request that had been pending across the
  crash was answered after the reinstall, inside its window.
- **UniFFI reads its metadata from the library's symbols, so generate the bindings before
  stripping.** The first product build: `build-android.sh` runs `llvm-strip` on every artefact it
  copies to `dist/`, and the product script pointed `uniffi-bindgen generate --library` at
  `dist/android-x86_64/liboffload_mobile.so`. The command printed nothing (`cargo run -q`),
  `app/src/generated/` stayed empty, and Kotlin failed with `Unresolved reference 'client'` on every
  import. Library mode finds the interface through `UNIFFI_META_*` symbols, which stripping removes.
  Pointed at `target/x86_64-linux-android/release/`, it generated
  `se/mach25/offload/client/offload_mobile.kt`, which then failed on `'message' hides member of
  supertype 'Throwable'`, from `MobileError::Daemon { message }`. Renamed to `reason`. The script
  now checks that a `.kt` file exists.
