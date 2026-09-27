# Rules, occurrences and reclaiming what they leave — full entries

The working rules are in `../rules-and-retention.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A rule's failed occurrence is picked back up behind the rule's back.** ADR-0020 §3 promises one
  occurrence at a time and `fire`'s own comment says what that prevents — "running it beside the
  first is two agents on one repository by a new route." Auto-resume is a new route:
  `recover_failed_runs` walks a per-node watch list that knows nothing about rules, and
  `rule_run_in_flight` reads `rules.last_run`, which by the time the backoff has elapsed names the
  *newer* occurrence. Measured: **49 firings, 105 resumes, 150 notifications**, two agents from a
  rule reporting one, each resumed leg three turns into an event half a minute old. The mild
  reading is two agents in one repository; the sharper one is that resuming *is* queueing, with the
  queue in another subsystem and the staleness unbounded, which is what §3 refuses to do. A firing
  withdraws the previous occurrence's recovery watch (ADR-0027), one line above the
  `reclaim_occurrence` that was already deleting its checkout on the same premise — so a nightly
  rule still resumes (nothing withdraws it) and a fast one does not. Fourth time a mechanism whose
  justification names a *person* has been re-read on the path where there is none, and ADR-0020's
  write-up predicted this one by name.

- **…and a peer retried them 23 times while the rule's node reported perfect health.** ADR-0027's
  own stated residual, measured rather than left as a note (ADR-0030): a rule on alpha, `accept =
  "never"` so every occurrence is placed on beta and fails there — **29 firings, 50 agent launches
  on beta**, bursts of five resumes in one second, and `0 events dropped` on alpha, because each
  occurrence finishes before the next tick so the rule's own bookkeeping is correct. ADR-0027's
  withdrawal is a **no-op** there — the watch is on beta and the rule is on alpha — so nothing
  bounded it at all, on the machine nobody logs into. Underneath it, a contradiction needing no
  rule and no gossip: `nobody_waiting` deletes a terminal occurrence's *checkout* precisely because
  nobody will come back for it, while `recover_failed_runs` intends to start an agent in that
  checkout. **Recovery of a machine-started run belongs to the machine that started it**, and the
  discriminator needs nothing new: `origin` **travels** (ADR-0024) and `runs.rule` deliberately
  **does not survive a merge**, so "tagged here" means "a rule on this machine fired it". Checked
  *before* attendance, because an occurrence is unattended by construction and a check after that
  branch is waved through every time.

- **A rule that fires all night has nobody to clean up after it, and `cleanup` was written for
  somebody who would.** `Supervisor::cleanup` is *deliberately* never automatic and its reason is
  right — when an agent finishes, its worktree holds the work, and reclaiming the disk the moment
  the process exits throws it away before anyone reads it. What ADR-0020 breaks is the premise
  underneath: **there is somebody who will read it**. A triggered run has none by construction,
  which is what unattended means, so the same rule unchanged left one checkout per firing for
  ever — measured, eight worktrees in forty-five seconds from a watcher ticking every three
  seconds. `reclaim_occurrence` is the automatic half, and every bound on it is a rule from
  somewhere else: only when the checkout holds nothing **uncommitted** (committed work is on the
  branch and survives teardown, so the fragile part is exactly what `git status` reports), only
  where the checkout **is** (an occurrence may have been placed on a peer, and `offload rm`'s bug
  is worse here because nobody is typing it), and asked of **git** rather than of the worktree
  summary beside the run, which is a turn boundary stale by construction. The general shape: a
  rule whose justification names a person stops holding on the path where there is no person.

- **A prune that gets one chance per record is a prune that mostly does not happen.** Everything
  standing between a spent occurrence and `delete_run` is a *temporary* no — a phone asleep for
  one tick, a delivery pass that has not run, a record the fleet is still gossiping — and a sweep
  that looks at each record once turns every one of them into a permanent yes-but-not-today. For
  the rule ADR-0020 §6 was written about, a watcher ticking every three seconds, it turns *all* of
  them into that: the previous occurrence is never quiet yet at the next firing. So a firing asks
  about **every** occurrence of its rule, which is what the `runs.rule` tag is for and why a
  single `last_run` pointer was not enough. The tag survives a gossip merge only because
  `write_run`'s `ON CONFLICT DO UPDATE` names the columns it overwrites and that is not one of
  them — nothing in the type system says so.

- **`live` is not a map of running agents.** Nothing removes an entry from it, so
  `live.contains_key(id)` means "this incarnation started that run at some point" — true of every
  run a long-lived daemon has ever hosted. `cancel` is the process handle and `release` clears it
  when the agent is gone, which is what `record_run` already reads to tell a live leg from a
  remembered one. Asked the wrong way, ADR-0023's checkout sweep reclaimed **nothing at all** on
  the one machine it was written for, and every test passed — because **a fixture's runs are never
  actually run**, so `live` is empty in all of them and the guard was vacuously true. The general
  shape: a guard that reads a map only production populates is a guard no fixture can exercise.

- **Reclaiming reaches only as far as the fact that justifies it travels.** Three sweeps in a row
  stopped at the same edge: `cleanup` is manual "because somebody will read the worktree", the
  only node that can tell an occurrence from somebody's run is the one holding the **rule**, and a
  rule is node-local. So a peer kept a record *and* a checkout per firing — measured, 81 → 181
  records in five minutes and 62 worktrees at 145 MB. The fix was not a fourth sweep but one bit
  on the record (`Run::origin`, ADR-0024): what travels is **not the rule** — a `RuleId` is one
  machine's name for one of its own things, useless to a peer for the same reason an `Audience`
  names services and never route ids — but the much smaller fact that a machine started it.
  `runs.rule` stays local for `except` and the `KEPT` column; `runs.machine_started` is the
  projection that travels, and the two sit in one `INSERT` with **opposite** rules about
  `ON CONFLICT DO UPDATE`: the local one must not be overwritten by a merge, the travelling one
  must.

- **…and a fourth time, for the checkouts — where "not the holder" is a trap.** Nothing removes a
  worktree when a run leaves; `remove` has one production caller and it is `cleanup`, manual and
  terminal-only. CLAUDE.md has said the first half since session sixteen as a note about `adopt`,
  and it is also a leak: measured, **62 firings left 62 worktrees and 145 MB on a peer in five
  minutes.** `reclaim_departed_checkouts` (ADR-0023) is the sweep, and the guard that says a run
  has *moved on* is the one to get right: **"not the holder" is true of every finished run**,
  because the lease goes with the terminal transition — a sweep built on it deletes the checkout
  `cleanup` is manual for. `Supervisor::describes` is correct and only for *this incarnation*
  (it reads the in-memory `live` map, so a restart says no about everything on disk).
  `RunProgress::by` is the durable form, and unstamped means unknown rather than somebody else.
  What it did not reach when it was written — an occurrence that finished on a **peer**, whose
  checkout is indistinguishable from work somebody is about to read — **ADR-0024 fixed two commits
  later**, and this entry went on saying otherwise for a session, as did the sweep's own doc
  comment four lines above the code that does it. Walked to settle it: every occurrence placed on
  a peer and finished there, and that peer's worktree count *oscillates* rather than growing. The
  general shape: a "does not reach" note is a claim with a date on it, and the commit that reaches
  it is not the commit that will remember to delete the note.

- **A node does not learn of its own run from somebody else.** The sibling of a rule in
  `gossip-and-merge.md` — a node must not believe a peer about its own absence — applied to its
  own runs *existing*. Pruning a record exposed it: every clause guarding a prune is about a peer
  that is **present**, and none of them can see one that is away. No partition needed, only a lid
  — a laptop sees an occurrence `Running` and closes, the home node finishes it and prunes it
  five quiet minutes later (quiet partly *because* the laptop is away), the laptop opens and
  gossips the copy it still holds. `merge_run` had never seen it, so it inserted it: a run held
  by *this node*, at an epoch nothing can contradict, with no agent behind it — lease renewed for
  ever by `heartbeat`, `ps` reporting last night's work as running, and a restart offering it as
  resumable. A record naming us as `home` for a run we do not have was made here and deleted
  here, so it is refused. Safe because the ordering is not a race: `hand_over` records a grant
  before publishing it and `take_run` saves before the arbiter confirms, so our own copy always
  exists first — and the seven fixtures that broke were every one of them introducing the home
  node's own run *by gossip*, which is a step the daemon has no code path for.

- **…and a quiet period buys the ability to re-ask, not safety.** A record deleted while the fleet
  is still gossiping it comes straight back through the merge path, *untagged*, so no future
  firing could ever prune it again — which is the whole reason to wait. Where there is no future
  firing the clause protects nothing and costs everything: `offload unwatch` on a three-second
  rule left **101** records behind, every one inside the window and none ever looked at again.
  `Prune::{AtAFiring, Finally}` is the distinction, and the general shape is that a delay whose
  justification is "we will check again later" is wrong wherever there is no later.

- **The outbox has two halves, and only one of them has a row.** "Is news still owed about this
  run" is not answered by the outbox alone: a row is created by a **scan**, so an event no route
  has reached yet is news with nothing to point at, and deleting it destroys the promise before it
  is made. `deliver::scanned_to` is the other half — the lowest run-log cursor among the routes
  that **exist right now**, because a cursor left behind by a peer that has gone never advances
  again and would pin every record for ever, while a route with *no* cursor holds nothing back at
  all (a new one initialises to the end of the log, so it will never want these events).

- **A record that looks like a leftover was the index that made the log reachable.** ADR-0021
  deferred pruning a bystander's copy of a finished run and *guessed* it "would change what
  `offload ps --all` and `offload logs` can answer". Measured instead (ADR-0025): a peer holding
  127 records with **0 events, 0 blobs and 0 outbox rows** — one kilobyte of row apiece — answers
  `offload logs <run>` **byte-identically** to the machine that ran it, because `logs` resolves
  the id in the local store and forwards to `RunProgress::by`. With no local row there is nothing
  to resolve and nowhere to forward. So deleting it would not *change* what a device can answer;
  it would end it, for that run, on that device. It is also the only answer available on
  ADR-0021 §7's standing: a node pruning **somebody else's** record cannot refuse the peer that
  teaches it straight back, so that prune deletes and re-learns for ever. The general shape: before
  sweeping something that looks like residue, find out what reads it.

- **…and what was missing was never a sweep — it was that nothing said so.** `offload rules`
  prints what a rule is *keeping* and `offload sinks` prints the queue behind a silence, and the
  machine these arrive on fastest has neither a rule nor a route: a peer that hosts nothing and is
  the one nobody logs into. `offload status` prints the count now, and the same sentence applies
  one layer down to a **checkout with no run record** — kept on purpose, possibly the only copy of
  an agent's uncommitted work, and pointed at by nothing until it was printed. Both are counts and
  not sizes: `offload status` is asked often, and walking every worktree to add up bytes would make
  the cheapest command in the CLI the one that touches the most disk.

- **A "does not reach" note is a claim with a date on it.** `reclaim_departed_checkouts` said "what
  this deliberately does **not** reach: an occurrence that finished on a peer" four lines above
  `nobody_waiting`, which is the code that reaches it — ADR-0024 closed the gap two commits after
  ADR-0023 stated it, and the sentence survived in the doc comment, ROADMAP, CLAUDE.md and
  ADR-0023's own consequence list. The commit that closes a gap is not the commit that will
  remember to delete the note, and four copies of a stale sentence is what reading rather than
  measuring produces. Settled by walking it: 160 firings placed on a peer, **76 records kept and
  flat, worktrees oscillating 0-5, disk flat at ~5 MB**, where each grew one per firing before.

- **A prune is node-local, and gossip is not.** Alpha's rule settles at 101 records while **beta
  goes from 81 to 181 in five minutes**, measured on two daemons — the peer learned every
  occurrence by gossip and nothing there will ever remove one, because the tag belongs to a rule
  on another machine. Not new: nothing has ever deleted a run record on a peer, for any run. What
  is new is that ADR-0020 made the supply automatic, so the accumulation moved to the machine
  hosting nothing. The fix is a retention policy for a finished run a node neither hosted nor
  submitted — a decision about *every* run, and one that changes what `offload ps --all` and
  `offload logs` can answer from a machine that did not run the work, which is exactly what
  session twenty-three had to fix. Unbuilt on purpose.

- **ADR-0020 §3 was guarded before an await and enforced after it.** `fire` walks the rules bound
  to a service and, for each, asks `Store::rule_run_in_flight` — *is this rule's last occurrence
  still going?* — because "running it beside the first is two agents on one repository by a new
  route". What the guard did not account for is what sits between it and the submission it
  authorises: `reclaim_occurrence(previous).await`, which is `git status --untracked-files=all`
  and, when the checkout is clean, `git worktree remove --force` followed by `git worktree prune`.
  Measured on this machine: **3.0ms and 4.1ms**, so the guard was taken at least 7ms before the
  thing it guarded, and more on a real repository.

  The interesting part is which racer survives. ADR-0027 put `stop_recovering(previous)` directly
  above the reclaim precisely so the *machine* cannot revive the occurrence — the recovery tick
  iterates the recovery map, and once the entry is gone it cannot decide to resume. That ordering
  is right and it is load-bearing. It does nothing at all about the other caller: a person typing
  `offload resume <previous>` has no schedule to fence and no entry to withdraw, and
  `Supervisor::resume` accepts a `Failed` run by name — `refuse_unresumable` lists `Failed` as
  resumable, and `reopen` is the first thing it does. `rule_run_in_flight` reads the `terminal`
  column, and `Pending` is not terminal, so one `reopen` inside the window is the whole of it.

  Measured with `tokio::join!` so the revival lands exactly where the firing yields rather than on
  thread timing: the firing submitted a second occurrence **and made it the rule's `last_run`**,
  which is worse than a stray run — the rule's own pointer now names the new occurrence, so the
  next firing's in-flight test asks about the wrong one and the revived occurrence is outside the
  rule's bookkeeping entirely.

  The fix is `in_flight`, asked twice, with the drop-recording inside it so the two calls cannot
  disagree about what "still going" means or about what `offload rules` is told — the drop reason
  is the only answer that command has to "why did nothing happen". A store error reads as
  **dropped** in both, which is the direction the single-call version already took: being unable to
  tell whether an occurrence is running is not permission to start a second one.

  Two things checked before this was believed, and one of them killed a bigger claim. The obvious
  wider reading — that an occurrence placed on a **peer** is revived by that peer's recovery tick,
  with the rule's node unable to call `stop_recovering` at all because recovery is node-local and
  never gossiped — is **already decided and built**: `standing_to_retry` answers
  `Standing::MachineStartedElsewhere` for an occurrence with no local rule tag, and
  `decide_recovery` escalates it as `NotOursToRetry` above every other question (ADR-0030). The
  deterministic version of this bug does not exist. And the test carries a control — the same
  fixture with nobody reviving the run, asserted to reach the submission — because every assertion
  above it would pass for a firing that simply never submits anything.

  Found by pointing the session's search at what a person and a machine both *write*: the state is
  `rules.last_run`, and the two writers are a firing and, indirectly, anything that makes the
  previous occurrence live again.

- **A precondition added for `offload when` was never added to `offload every`.**

  **How it was found.** `offload every` came off `docs/DEMO.md`'s mention-count loop. Its
  refusals are all good — `every 30s` is refused with a reason worth reading (*"an occurrence is a
  record, a notification and possibly a bill"*), `every banana` names the units, and `--at 20m` on
  a 15m period is refused as naming no instant. Then:

  ```
  $ offload every 1m --task nosuch
  schedule e4ad9b43b1437a5e
  first tick in 49.8s
              it is the fleet's now: whichever device is available fires it
  ```

  Nothing else. The control, one command over, had warned since ADR-0032:

  ```
  $ offload when othersvc --task nosuch
  rule 61c8a82c5a8b6db4 — on `othersvc`
    …
    Nothing in this fleet nominates a task for `nosuch`, so a firing will be refused rather
    than run — `offload rules` counts those under DROPPED, with the reason.
  ```

  *(An aside worth keeping: `--task` takes a **service**, not the `[[tasks]]` block's `id`. A
  first pass here used the id and read the resulting warning as a false positive. The id is
  required and is not the name you schedule.)*

  **What then happens.** The tick fires, the bid round refuses it, and the daemon logs at INFO:

  ```
  nobody took this occurrence: no node will take this run
    alpha        ineligible: has nosuch as execute (nothing offers it)
  schedule=e4ad9b43b1437a5e run=01a08f2e2080… tick=1789108560000
  ```

  — a dozen attempts per tick, every period, for as long as the fleet exists. And
  `offload schedules` says:

  ```
  e4ad9b43b1437a5e  every 1m0s
      task nosuch
      next      in 52.5s  ·  nothing fired from here yet
  ```

  which is the sentence it also prints for a schedule created ten seconds ago. There is no state
  that separates the two: `schedules.last_tick_ms` is the node-local high-water mark and is set
  **after** a successful placement, so a tick that was attempted and refused leaves the row
  identical to one that never came.

  **The fix, and the part of it that is a decision.** `Request::Every` now computes the same three
  notes `Request::Watch` does, from the same functions — and the comment directly above that arm
  is *about this class of omission on its neighbour*, which is worth noticing: reading it did not
  prompt anybody to check the arm underneath.

  The precondition sentence is **not** shared, and that is deliberate. `task_note` ends *"`offload
  rules` counts those under DROPPED, with the reason"*, which is true of a rule and false of a
  schedule — nothing counts a refused tick. Borrowing it would send somebody to a report that will
  never mention their problem, which is worse than the silence. So `nominates_task` and
  `missing_resources` are shared (the facts, which must not diverge) and
  `schedule_precondition_note` words the consequence (which differs). The unit test's load-bearing
  assertion is the negative one — the note must **not** contain `offload rules` or `DROPPED` — and
  it fails on the pre-fix behaviour with the function returning `None`.

  ```
  $ offload every 1m --task nosuch          # after
  schedule 67373d926b902b39
  first tick in 4.3s
              it is the fleet's now: whichever device is available fires it
    Quiet while it works: you will hear about a failure or a question, and not about a tick
    that went fine.
    Nothing in this fleet nominates a task for `nosuch`, so every tick will be refused rather
    than run. `[[tasks]]` in a node's config nominates one, and the next tick after that will
    work.
    Nothing counts a refused tick, so `offload schedules` will go on
    saying nothing has fired from here.
  ```

  The last two lines are a promise, and it was measured: seventy seconds and one tick later,
  `offload schedules` did say exactly that. The control is the same command for `--task push`,
  which this node nominates, and which prints none of it.

  **The general rule.** When a mechanism grows a second front door, the thing to grep for is every
  caller of the *predicate*, not every use of the type — the type compiles either way. And a
  warning is only worth printing if the report it points at will actually say something; where no
  report will, say that instead.

- **The two removal commands prune differently, and the asymmetry is correct.**

  `offload unwatch` calls `prune_occurrences(Some(rule), None, Prune::Finally)` on its way out and
  takes the records with it — measured, 115 records to 14 in one call, 101 of them the occurrences
  of a rule firing every three seconds. `Prune::Finally` sets the quiet-period bound to
  `i64::MAX`, which is the entry above this one: where there is no future firing, a delay
  justified by "we will check again later" protects nothing.

  `offload unschedule` prunes nothing, and should not. A schedule is **gossiped**, so a peer is
  still teaching this node those occurrence records for `GOSSIP_TAIL` — and a record deleted
  inside that window comes straight back through `merge_run` *untagged*, therefore unprunable by
  anything, for ever. What collects them is the fifteen-minute retention pass in `main`, asked
  with `rule = None` and `Prune::AtAFiring`; because that pass recurs whether or not anything
  fires, the quiet period there buys a real re-ask rather than a hope.

  **Measured rather than read off the pass**, because a retention claim inferred from a
  `Duration::from_secs(15 * 60)` is the kind this file exists to distrust. One daemon, two
  tombstoned schedules and a removed rule, polled every 20s: `records=22` at 07:41, `records=8`
  at 07:55:23 — fifteen minutes after daemon start, on the nose. The 8 that stayed each have a
  reason: `select state, machine_started, count(*) from runs group by 1, 2` gives 7
  `failed`/machine-started, held deliberately by `spent_occurrences`' `state = 'completed'`
  clause because a failure is the record somebody comes back for, and 1 `completed` run with
  `machine_started = 0` — an operator's submission, which that query never considers at all. So
  the pass collected everything it should and nothing it should not; nothing leaks, and the
  fifteen minutes is the cost.

  So the two commands differ because the two tiers differ in exactly the way the heading of this
  section says they do. Walked on one daemon, both commands, and recorded here because "make the
  two removal paths consistent" is a plausible-looking change that would reintroduce the
  come-back-untagged bug on the gossiped half.

*The twelve entries below were backfilled in session ninety-one from `docs/sessions.md` and the
commits that added each rule (`dcc4e2f`, `b445fd7`, `1d7db78`, `bd234fd`) — sourced, not
reconstructed from the rule text.*

- **The occurrence's record is not enough to say a tick has fired, because the record is deleted.**

  Session sixty-seven, ADR-0056's build (commit `dcc4e2f`). The ADR's first draft said *"the
  occurrence is the cursor"* and argued it well: a cursor would be a mutable gossiped field
  describing something the fleet already records with an owner and a merge rule. Reading the
  retention pass killed it. A scheduled occurrence is machine-started, so `prune_spent_records`
  reclaims it about an hour after it finishes — for a **daily** schedule, twenty-three hours before
  its tick ends. A nightly schedule would have fired hourly. The fix is a **node-local** high-water
  mark, `runs.rule`'s arrangement, which needs no owner because no peer can state it.

- **A schedule is not due for the tick it was born in.**

  Same build, measured on one daemon one command apart: `first tick in 36.9s` printed at the
  keyboard, and an occurrence in the log five seconds later. The tick containing the moment of
  creation started *before* it, so `every 15m` produced two occurrences eight minutes apart, and
  the report was wrong about the one thing somebody had just asked. Compared against `created_at`
  — immutable and gossiped — so a peer that learns the schedule declines the same occurrence the
  creator did.

- **A refusal must not consume the tick, and a retry must not be unbounded.**

  Same build, measured on two daemons: the home killed, the successor correctly becoming steward
  and firing, and nobody able to host because the successor was still inside its fifteen-minute
  probation. The tick had been marked *before* the submission — the ordering that cannot
  double-fire — so a daily schedule would have lost the day to a refusal that lasted seconds. The
  mark moved after the placement, bounded by a dozen attempts. **Attempts, not elapsed time**: the
  first bound was a window from the start of the tick, and the first test written against it fired
  nothing, because a daemon that starts mid-tick has made no attempts and should serve it.

- **A gossiped set needs a tombstone.**

  Same design. A deleted row comes straight back from the next peer, and the schedule then fires
  for ever on whichever node was last to hear. `Revocation`'s shape and `Revocation`'s argument.
  Walked in session sixty-nine — see the two entries on *away* and *forgotten* below.

- **Two nodes firing one tick is one *record* and would be two *agents*.**

  Same design. Who fires a schedule is `arbiter_for`'s rule — home while available, else the
  lowest-id available node — factored as `ClusterView::steward_of` rather than restated. The
  occurrence's `RunId` is derived from `(schedule, tick)`, so two stewards converge on one record;
  but that is *the net under a transiently doubled steward and never the mechanism*, because one
  record can still be started by two nodes. The walk read the derivation directly: the occurrence's
  prefix `01a08ad3ee20` is `1789035540000` in hex, the tick itself.

- **A rule can fire either tier, and the *event* stops at the boundary.**

  Session sixty-seven (commit `b445fd7`). An agent rule appends the trigger's line to its prompt
  under a heading that says it is data; a **task is not told the line at all**. That is ADR-0019 §1,
  not laziness: a trigger's output is text from outside, and handing it to a nominated program as
  `argv` is exactly the submitter-supplied command line that ADR refuses most strongly. If the
  payload is ever wanted, stdin or one named environment variable is a decision with its own ADR.

- **`SubmitTaskRequest` had no `origin`, which was invisible until a machine could fire one.**

  Same commit. The agent request had carried ADR-0024's origin since it existed; the task one had
  not, which mattered only once a rule could fire a task. The occurrence would have been recorded as
  an **operator's**, so nothing would ever reclaim its record, and `cleanup`'s premise that somebody
  will read the worktree would have applied to a run nobody typed. The compiler found it — the good
  case.

- **A rule's stored shape is a storage format, and no version number covers it.**

  Same commit. A rule is a whole `RuleSpec` as JSON in `rules.request_json`, so the `Work` split
  changed what a daemon reads back from its own disk — no migration to hang it on, no wire version
  to protect it. Pinned with a **hand-written** compatibility fixture, because a generated one
  changes shape in lockstep with the code it is meant to pin. The same hazard as session
  sixty-six's `RunSpec` split, one type along.

- **A precondition is a refusal on one command and a note on the other, and the fact underneath is
  one function.**

  Same commit. `offload run --task push` is refused at the keyboard; `offload when --task push` is
  written with a warning, because a rule fires for months and the machine that will nominate the
  program may not have enrolled yet (ADR-0032, ADR-0036). Both read one `nominates_task`, asking
  this node's config and the fleet's advertised `Role::Execute` capabilities. Walked including the
  note's promise: three firings refused, and `offload rules` counted them under DROPPED with the
  round's own sentence.

- **A schedule survives its device being *away*, not its device being *forgotten*.**

  Session sixty-nine (commit `1d7db78`), the tombstone walk on two daemons. Removal typed on the
  *peer* reached the home in eight seconds; removal typed while the peer was killed reached it when
  it came back, and the peer fired nothing in between. But a daemon restarted while the home is off
  has a view with no peers, so `steward_of` answers `None` and nothing fires — right (a node that
  has met nobody must not fire the fleet's schedules) and self-healing: the schedule resumed on its
  own once the home returned on the seed backoff. ADR-0056's *"a schedule survives the device that
  created it"* holds while that device is **remembered**; the ADR carries the amendment. The same
  `None` for a *run* is `explain`'s `ArbitratedBy(None)`, walked session ninety-one.

- **…and any node may write a schedule's tombstone, which is what makes a retired home removable.**

  Same walk and commit. ADR-0056 §5 said the home owns `removed_at`; the code never checked, and the
  code is right — the merge is a minimum over a monotonic field, so it converges whoever sets it,
  and a tombstone only the home may author cannot be written once that laptop has been sold.
  Amended in the ADR; the precedent is `Revocation`, which travels for the same reason.

- **A rule or a schedule bound to a task nobody can *run* is a different silence from one bound to a
  task nobody nominates, and only the second was reported.**

  Session eighty-one (commit `bd234fd`, ADR-0019 amended). `nominates_task`, which feeds both
  `offload when --task` and `offload every --task`, asked only whether a program was *nominated*.
  Without asking `authenticated` on this node and every peer, both commands accepted a rule and a
  schedule for a nominated-but-unrunnable program **in silence**, refused at every firing for ever,
  while the control (nothing nominated) printed the whole paragraph — the silence session
  seventy-three removed from `offload every`, arriving again through the same predicate.
  `task_refusal` now answers three ways (`task::Nomination`), because *nothing nominated* and
  *nominated and unrunnable* have different fixes.
