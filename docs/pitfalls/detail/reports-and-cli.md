# Reports and the CLI — full entries

The working rules are in `../reports-and-cli.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **A command names a run, and a run is on a machine — so it acts there or it says it cannot.**
  `offload approve` and `offload deadline` route (to the holder, to the field's owner); `offload
  cancel` and `offload rm` did not, and each was worse than a refusal. Cancel read this node's
  *process table* and reported "run ab12… is not running" about a run running perfectly well on
  the desktop, spending money, with nothing forwarded and the operator told it had stopped. And
  `rm` **succeeded**: `WorkspaceManager::remove` returns `Ok` for a path that is not there —
  correct, teardown is idempotent — so `cleanup` went on to write `workspace: "removed"` for a
  checkout on another machine. Those numbers gossip, and `accept_progress` breaks a tie on the
  author's clock, so a copy matching the holder's counters with a later stamp is *ahead* and
  wins everywhere: the fleet reporting a worktree gone while it sits untouched on the desktop.
  A finished run names no node — the lease goes with the terminal transition — so "is it here"
  is a question only this node's own disk can answer, and `Removal::{Removed, NothingHere}` is
  it being asked. **Cancel travels now** (wire v21): the target comes from the record, and there
  are two kinds of it — a run somebody is *holding* is on that node, agent or no agent, and a run
  nobody holds is a record belonging to the node that arbitrates it (the same `arbiter_for` a
  `SpecEdit` uses). The process table was never the question; it cannot tell a run that is
  elsewhere from a run that is here and has not started, and it answered both "not running". A
  run whose holder has gone quiet is refused rather than forwarded into a timeout: `Orphaned` is
  an observation, an agent may well still be running there, and the hold-down reaches whoever
  answers next. **`offload checkpoint` travels too** (wire v22) and has only the first kind of
  target: a turn boundary is a moment in a *process* (ADR-0004), so a run nobody holds has none
  coming and is already back in the pool, which is where a checkpoint would have put it. Its old
  refusal — "it is not running here, so there is no turn boundary coming" — was the subtler
  version of the same failure: true about this machine, silent about the one the run is on.

- **A run nobody holds still has a log, and it is the arbiter's.** `logs` resolves a run to its
  holder and falls back to `arbiter_for` — because a pending run's one message is the arbiter
  saying it is not going to make its deadline, and resolving to "nobody" would hide exactly the
  thing worth reading. Safe to follow because a node that is not the holder reports its leg as
  ended, so the poll stops rather than waiting on a node that will never produce a turn.

- **…and it must not be written when the state it names was refused.** `Run::cancel` is unfenced
  and still refuses a terminal run, and the window where that happens is the one somebody is most
  likely to be typing in: the agent reports its result, the run goes `Completed`, the stream has
  not closed yet, and the pump's *biased* select picks up a cancel that arrived a moment ago. The
  transition was refused, correctly, and the log line was written anyway — so a run that had just
  succeeded had `cancelled` as the last word in its own log while its record said `completed`.
  Written only when the transition applied; nothing on the refusal, because whatever moved the run
  to its terminal state logged that on the way past.

- **A log kind named after a state must not be borrowed for an operation inside it.** A failed
  *capture* was logged as `LogKind::Failed` — the run's terminal state — and everything reading
  the log believed it: `is_terminal` hung up every follower mid-run, taking the attendance that
  decides whether a failure resumes itself with it, and the notification projection told somebody
  their run had failed a minute before telling them it had finished. `CaptureFailed` is its own
  variant now. A log is read by things that cannot ask what was meant.

- **A command that reports a policy has to report the policy in *force*.** `offload policy` — the
  command whose entire job is "would this device take work right now?" — computed
  `WorkPolicy::for_class(class)` and answered from that. So the moment an owner wrote a `[policy]`
  block, it described a policy their daemon does not use: a phone configured `accept = "always"`
  was told it accepts work only while charging, and a battery floor of 80 was reported as 40. The
  natural conclusion is that the fleet is ignoring you. It takes `--config` now, the same path
  `offloadd` takes, and it goes through `Config::work_policy` rather than a second copy of the
  layering — and it *says which* policy it is answering about, because a class default presented as
  the node's is the whole bug in one line.

- **A default that differs from the neighbouring command's has to be said where it is chosen.**
  `offload when` prints which way `--notify-on` went and `offload rules` prints `reports problems
  only`, because a rule written months ago is exactly where **"no notification" and "no failure"
  look identical** — and a watcher that fires and succeeds is now silent, which is also what a
  watcher that is not firing looks like. That is what `fired`, `dropped` and `kept` are for.

- **Run ids are UUIDv7, so their leading bytes are a clock.** Bytes 0..6 are a 48-bit
  millisecond counter, so bytes 0..4 are the *top* 32 bits of it and hold still for 65_536 ms:
  every run submitted within about a minute shares its first 8 hex characters. Never use an
  abbreviated run id as an identity — branch names use the full id, and displayed prefixes are
  **12** characters, which is exactly the whole clock. That rule lived in the CLI and not in
  `RunId::short`, which printed eight, so the daemon's own refusals named two runs at once
  ("run 01a02606 cannot be cancelled") beside a CLI that had printed twelve. It is one number in
  `byte_id!` and it applies to `RunId` alone: a `NodeId` is an ed25519 key and a `BlobHash` a
  BLAKE3 digest, whose leading bytes are uniform.

- **…and it must not report what only a daemon could know.** The same sentence from the other
  side. `offload fleet` passed `health` an empty certificate list, which the notes read as "met
  nobody, therefore no approver about" — so a laptop gossiping with an approver every second was
  told its fleet had none and pointed at `offload grant approve`, which needs the passphrase,
  which is the posture the invite path exists to avoid. `offload status` on the same machine said
  `1 approver(s)`. The caveat that would have softened it was printed under
  `state.approver.is_some()`: the only node ever shown it was one that is itself an approver, and
  the node being misled is the one that is not. `Met::{Handshakes, NotAsked}` tells the callers
  apart, because the difference is in who is asking rather than in the data — a daemon's empty
  list is a fact, a CLI's is an absence of evidence.

- **`Millis`'s `Display` renders a duration, so an absolute instant printed through it is always
  wrong.** `offload status` reported a rate limit's reset as **`496117104h5m`**. `offload-core` has
  no clock and therefore cannot format a wall-clock time at all, which is why the sentence there
  carries no instant and the readable version is built where there is a clock
  (`supervisor::describe_refusal`, `when::until`).

- **A report has to say what the decision says.** Three of these in one sweep, and they are one
  bug wearing three hats: `offload policy` computed the class default while the daemon used the
  configured policy; `offload probe` printed `Battery { percent: 96, charging: true }` where the
  gate reads `on_mains()` (charging *is* mains, so the line invited somebody to think a battery
  floor applied when it did not); and `offload fleet --socket <path>` answered from
  `$HOME/.offload` because `--socket` picks a *daemon* and membership is read from the state
  directory. Each was silent, confident, and about the wrong thing. When a command exists to tell
  somebody what the system will do, the value it prints has to come from the same place the system
  reads — and where a flag cannot apply, saying so beats ignoring it.

- **A run nobody holds got there two ways, and one fallback covered both.** `offload logs`
  resolves to the holder and falls back to `arbiter_for` — right for a *pending* run, whose one
  message the arbiter itself wrote. A *finished* run also names no node (the lease goes with the
  terminal transition), and there the arbiter is precisely the machine that never had the log; when
  the arbiter was the asking node the answer was **silence**, exit 0, which reads as "this run
  produced no output". An overnight run that finished on the desktop was invisible from the laptop
  it was submitted on — 56 lines there, 0 here, measured. What knows the answer is
  `RunProgress::by`, the leg that last wrote the run's *position*, because that leg is by
  construction the leg that wrote the log. The general shape: when a value is `None` for two
  different reasons, a single fallback is right for at most one of them.

- **A countdown is not a fact about a run that has stopped running.** `offload explain` read the
  deadline against `now` whatever the state, so a run given `--deadline 1m` that finished thirty
  seconds inside it reported `due overdue by 31.2s`, two lines under `state completed — finished
  1m25s ago`, and the number grew for as long as the row was kept. The no-deadline half was the
  same mistake more quietly — `waiting 1m58s`, still counting, about work that was over. `ps` was
  right the whole time, from the same daemon and the same field, because `RunSummary.due` filters
  on `!state.is_terminal()`. Once a run is over the question changes from how long it has to
  whether it made it, which `Run::prospect_at` answers at the instant it ended — and answers for
  the run nobody gave a deadline too, rather than reporting it as infinitely late.

- **A command that answers "why is it not running" has to answer it in every state, and
  `assigned` was the one it could not.** `offload explain` printed the lease's remaining time and a
  canvass line reading `already holds this run` — true, and not the question, because a canvass asks
  who would *take* the run. Invisible while every reason a held run waits was seconds long (a slot
  frees, a budget drains); ADR-0029 made one of them hours long, and then the one sentence that
  explained it was printed by `offload run` at submission and nowhere afterwards.
  `Supervisor::start_refusal` is the answer and it is the **same call** `start_held_runs` makes, so
  the report cannot disagree with the gate and a new reason arrives without being enumerated. And
  the footer said the opposite of the truth: `No node would take it right now` fires when nobody is
  bidding, which is exactly what a canvass returns for a run somebody already **holds** — the
  holder answers `already holds this run` and everybody else is refused for having nothing to do
  with it.

- **…and it said "nobody is watching" about a run it was keeping *for* the watcher.** The same
  command, the adjacent state. `attendance` was sampled **now**, and ADR-0013's module docs give
  the reason that is always wrong for a failed run: the observation is taken at the moment of
  failure *because* a failure ends the stream, so sampling afterwards "would find nobody watching
  every single time". Measured — the daemon's `reason=somebody was watching it when it failed` and
  `explain`'s `attendance unattended`, one second apart, one node. For a terminal run the
  remembered observation wins and says so; where the node has forgotten it says *that*, which is
  `AttendanceUnknown`'s answer and its reason. And the other half of the same gap: nothing said
  whether a failed run was coming back, though `failed` looks identical whether a retry is thirty
  seconds off, the retries are spent, or it was left for the person who was watching. Both needed
  the escalate branch to **record** its decision rather than delete the entry — deleting it was
  right about saying the reason once and was also the only record of it. The general shape:
  **deleting state to stop something being repeated also stops it being answered for.**

- **A header and its rows are one table, so they must not be two widths.** `RunId::short` went
  from eight characters to twelve, `offload ps` widened the row from ten to fourteen, and the
  header stayed at ten — so the most-read command in the CLI printed every column one heading to
  the left of where it belonged, for two phases. One `const` shared by both lines is the fix; two
  numbers that agree only because somebody remembered are two things to remember.

- **A bid describes one second, so nothing replays one.** `offload explain` re-asks the
  fleet (`Cluster::canvass` — the same round, granting nothing) rather than showing the bids
  that placed the run: a node that was full at submission is not full now, and a stored bid
  presented as current is a confident wrong answer. Silence stays its own verdict there —
  "would not" and "did not answer" are different facts, and a node mid-restart has not
  refused anything.
- **A verdict reads from the right function and can still describe the wrong branch.**
  `offload explain`'s `verdict` calls the same `offload_core::supervise` the supervision loop
  runs, precisely so that the explanation cannot drift from the decision — the rule this file
  keeps. It drifted anyway, one level down: the `Bystanding::Unheld` arm rendered as *"nobody
  holds it, so it is waiting for a bid round rather than for a hold-down"*, and waiting for a bid
  round is what `Supervision::Place` does. `Unheld` is the `else` of that test — the arm reached
  when no round is coming for the run at all. The sentence was inherited verbatim from
  `Bystanding::Unheld`'s own doc comment in `offload-core`, which was written when nothing ever
  placed a pending run without a person and was therefore true; ADR-0014's queued runs took those
  words for the sibling branch and left this one asserting them. Found while walking ADR-0041's
  seam, where it does real damage: an operator who has just drained a node is told the fleet is
  about to take a run that nothing will offer it. Two sentences now, because the two ways to reach
  `Unheld` end differently — a checkpointed run is parked and waits for `offload resume`, and one
  that never ran was refused at submission and not queued, so nothing offers it again. Both the
  CLI string and the core doc comment were fixed, since the second is where the first came from.
  **The guard asserted the wrong string.** The test — named for a pending run saying it waits for
  a round and not for a timer — required the words "bid round", while its own comment described
  the distinction it meant to protect: placement, not patience, which the corrected sentence still
  makes. A test that pins a sentence rather than the claim behind it holds the mistake in place,
  and this one went red for the fix and had to be read before it could be believed.
- **A count that means two things gets one sentence, and it is right for at most one of them.**
  ADR-0035 split `finished` out of a drain's `left` for exactly this; ADR-0041 split `later` out
  of what remained, one outcome further on. `left` means *final* — nobody in the fleet would take
  it — and it was also carrying "still mid-turn at the deadline", which is not an ending at all.
  The two are told apart by nothing the operator can see, and the advice printed under `left`
  ("`offload ps` and `offload nodes` say which") cannot answer the second: what that run waits on
  is a turn boundary that has not happened yet, which is in neither of those commands. The
  rendering now says what will happen — `1 run(s) still mid-turn — this node hands each over at
  its next turn boundary` — and it is the last line of the report, because it is the only one
  about the future.
- **A refusal that will never lift must not be logged as one that will.** `start_held_runs`
  matched every failure from `start_run` with one arm — `tracing::warn!(.., "could not start it
  yet")` — under a comment that says the run is "left `Assigned` deliberately: the next tick tries
  again". For a run that ended while it waited, both halves are false. `held_but_not_started`
  filters on `RunState::Assigned` and a terminal run holds no lease, so it never appears again;
  nothing tries anything, and the last thing written about it promised somebody that something
  would. Not a hypothetical arm either: it is exactly what the cancel-during-a-start race produces
  once that race is *fixed* — the fix makes the cancel stick, and this line is how the tick then
  reports it. `SubmitError::Ended` is its own variant for the same reason `LostTheRun` is, which
  its doc comment already argues: the caller has to tell a refusal that will lift from one that
  never will, and no amount of reading a formatted string does that. Two deliberate restraints in
  the shape of the variant. Its `Display` is character-for-character the `Refused` it replaced,
  because nothing about a *person's* reading of "it was cancelled while it waited to start"
  changed; and the level drops from `warn` to `info`, because a cancel that worked is not a
  warning — the run did what it was told. The guard asserts the variant *and* the sentence, so
  neither can drift into being the other's justification.

- **…and a held run publishes its refusal into the worktree column, so something has to take it
  out again.** ADR-0006's accept-without-starting means a run can be this node's, with a lease and
  an epoch, and have no worktree at all — so `take_run` writes the *refusal* into the column that
  would otherwise describe a checkout, which is the right call and is what the `summarise_refusal`
  entry above is about. What nothing did was retract it. `Run::cancel` moves the state and touches
  no progress, so the last thing the column was told stood: measured on a daemon, `offload ps`
  showed `cancelled` beside `waiting for a slot` for as long as the record existed, and the same
  run's `offload rm` said "no checkout for … on this node". Two reports about one run, disagreeing
  about whether there was ever a directory. And the column **gossips**, so on a real fleet that is
  every node's answer, not one machine's display bug. `NEVER_STARTED` is written on the branch of
  `cancel_run` that already knew — the `assigned` arm, which is the one whose note to the operator
  is "it had not started, so nothing was interrupted" — and deliberately not on the others: a
  `Running` row reaching that function after a daemon restart has a checkout on this disk, and
  whatever the last leg said about it is still true. Walked both ways in one pass to check the
  discrimination rather than assume it: the held run came out `cancelled` / `never started` with
  no worktree directory on disk, and a *running* run cancelled a minute later came out `cancelled`
  / `clean`. It goes through the same width-and-no-digits guard as the refusals, because it lives
  in the same eighteen columns and outlives its moment the same way.

- **Terminal is three outcomes, and the nicest one is not the default.** `RunState::is_terminal()`
  is true for `Completed`, `Failed` and `Cancelled`, so a branch that tests it and then states what
  happened is stating one of three things and guessing which. Written into ADR-0041's owed-handover
  pass as `Owed::Done("it finished here first")`, and measured wrong within an hour of shipping:
  `offload cancel` on a run the node owed the fleet logged `why="it finished here first"`, and so
  did a run whose agent died without reporting a result. That log line is the *only* record of what
  became of a run the drain explicitly promised to hand over — the drain's own report was printed
  minutes earlier, and the run never reaches the fleet — which makes it the worst available place
  to guess. It is also the same rule this file already carries twice, arrived at from a third
  direction: `wait_for_checkpoint` answering `None` for three reasons, and `left` counting two
  outcomes with one sentence. Each state now says its own thing, and a terminal state added later
  falls through to `RunState::name()` rather than inheriting one of the three — which is precisely
  how this went wrong, since `is_terminal` grew a third variant and the sentence beside it did not.

- **A verdict built on the right function still guesses, if the thing that decides is not in the
  function.** `offload explain`'s verdict calls the same `offload_core::supervise` the supervision
  loop runs once a second, deliberately: an explanation computed by different code from the
  decision it describes is a second implementation that disagrees quietly, on the day it matters.
  That rule is right and it is not the whole rule.

  The handover debt a drained node keeps (`Mesh::owed`) is node-local, in memory, and gossiped
  nowhere by design — so no pure function of `(view, run, now)` can see it, and `supervise` is
  correct to return `Bystanding::Unheld` for a run that has one. What the report then did was infer
  from what was left on the row, and what is on the row is a checkpointed `Pending` run, which is
  exactly what `offload checkpoint` leaves. So a run this node was re-offering on every backoff was
  described as *"parked with its checkpoint, and waits for `offload resume` rather than for a bid
  round"* — sending an operator to do by hand the thing already in progress, about a run nobody had
  parked.

  The fix is to hand the fact in rather than let the sentence derive it. `Observed` exists for
  this, and `departing` is the precedent one field back and the same shape: a drained node will not
  start an agent, and an explanation that does not know it is draining tells somebody their run is
  about to be picked up by a machine that has stopped doing that. `Mesh::owes` reads the same set
  `finish_owed_handovers` walks, so the sentence and the pass cannot drift.

  Why it shipped: the arm was correct for both cases that could reach it when it was written, and
  its guard asserts the sentence for those two. A third case arriving is not something a test of
  the first two notices — the same reason `Bystanding::Unheld` was still carrying a sentence about
  bid rounds after ADR-0014 gave those words to its sibling branch.

- **…and whether anything will try again is not a property of the error.** Classifying the tick's
  failures by error variant is the obvious way to tell a permanent refusal from a temporary one,
  and it is wrong the way taxonomies are wrong: right about the cases somebody enumerated. `Ended`
  covered the terminal row. Measured over every way `assign` can refuse inside `hold_here`, two
  were left. A run that had **moved to another node** while the tick worked through the one ahead
  of it came back as `agent: cannot assign a run that is running` — a `TransitionError` stringified
  into an operator's error message — logged under `could not start it yet`, which nothing would,
  since the run is not this node's any more. And a `Pinned` run with an attempt spent produces
  `cannot reassign a run that is pinned`: equally permanent, equally mislabelled.

  Two facts, and the fix is to stop treating them as one. **What went wrong** is the error;
  **whether the tick comes back** is the tick's own filter, so ask the filter.
  `Supervisor::awaits_a_slot` is the three conditions `held_but_not_started` selects on, written
  once and now asked by both, and `will_try_again` reports on exactly what the loop selects on — so
  a failure mode nobody has thought of yet cannot be described wrongly by default. Sound only
  because of the registration fix in `lifecycle-and-recovery`: while a failed start could leave its
  `live` entry behind, this predicate would have answered `false` for every failure, which is the
  wedge reporting itself as a decision.

  `LostTheRun` is the sentence for the moved case — the variant whose doc comment already says the
  caller has to tell this apart from everything else, used here for the *reading* rather than for
  the fence. The pinned case gets a readable `Refused` and **not** a variant of its own: three
  gates stand in front of it, since `release` refuses to return a pinned run to the pool,
  `reassign` holds it with `HoldReason::PinnedToHolder`, and a drain keeps it with
  `KeepReason::PinnedHere`. Nothing offers a pinned run to anybody, and a variant for a state the
  product cannot produce is a taxonomy growing to cover a case rather than a measurement.

## …and the better fix was to stop the decision from being blind, not to feed the report

The entry above is right about the shape and settled for the smaller half of it. `offload
explain`'s verdict is `offload_core::supervise`'s on purpose; the debt a drain kept was in memory
and no pure function of the view could see it; so the fact was handed in as an `Observed` field,
the way `departing` already was. That fixed the sentence it was written for and nothing else.

What it could not fix: the *other* readers. The daemon's own supervision loop logged `queued run
placed` from `Supervision::Place`, which was true of the only run that could reach that arm and
became false as soon as a second way in existed. So did the CLI's advice under `Drained::left` —
"`offload ps` and `offload nodes` say which" — about a run whose future was a turn boundary that
had not happened.

Putting the fact on the run (ADR-0042) put it inside `supervise`, and the decision then carries
its reason: `Supervision::Place(Offering::{Queued, LetGo { by }})`. The verdict, the state line
and the log all read one value, the `Observed` field was deleted, and the arm that had to be told
separately no longer exists. **When a report has to be told something the decision cannot see,
ask first whether the decision should see it.** Sometimes it genuinely should not — `departing`
is a live property of this process and belongs on the side — and sometimes, as here, the thing
that made it invisible was a choice about where to keep it.

## A count that has been split once will need splitting again

`Drained` has been split three times now. `finished` came out of `left`, because a run that ended
by itself was being reported as one nobody would take. `later` came out of `left`, because a run
mid-turn at the deadline is not final. And `pooled` has come out of `later`, because the two
things `later` covered stopped waiting on the same thing the moment the record could carry the
fact: a mid-turn run needs this process to reach the boundary it will be handed over at, and a
released one needs no daemon at all.

That last distinction is not a nicety. It is the answer to the question somebody typing `offload
drain` is actually asking, which is whether the lid can be closed — and it was one number with
one sentence over it.

The habit worth taking from three repetitions: **when you teach the daemon a new way for a run to
end up somewhere, look at every count that already covered the old ways.** None of these was
found by a test of the arm it belonged to; each was found by reading the report after the
behaviour changed under it.

## A note about why this node has not started a run outlives the node's intention to

`Supervisor::take_run` accepts a run it has no room for (ADR-0006) and writes the reason into
`RunProgress::workspace` — `waiting for a slot` where the run is merely behind another, or the
summarised refusal (`at capacity: 1/1 runs`, or an account rate limit) where there is one. That
field is the WORKSPACE column of `offload ps`, and its own doc comment records why it carries the
specific reason rather than a generic sentence: it **gossips**, so a vague answer is the whole
fleet's answer.

Nothing cleared it when the commitment was handed back. `release_unstarted` writes the release,
bumps the epoch, returns the run — and leaves the note, so a run that is `pending` with no holder
anywhere still says why *this* node had not started it. Found while walking `GiveBack`, on the
build where the run was deliberately stranded:

```
09:33:02  runs 0/1 · accepting yes
          01a05194d968   pending   0   -   -   -   waiting for a slot   the commitment
09:34:42  runs 0/1 · accepting yes
          01a05194d968   pending   0   -   -   -   waiting for a slot   the commitment
```

Two minutes, on an idle accepting node, about a run waiting for nothing of the sort. It is
transient on the fixed build — the run is placed on the next backoff — but transient is not the
same as right, and the drain's `pooled` case leaves it standing for as long as the fleet has no
room.

**Cleared in `release_unstarted`**, which is the one place both callers go through: the drain and
the late-commitment pass are both this node ceasing to intend to start the run, and a fact hoisted
into "the caller" has as many owners as that function has callers — the same argument that put
`stop_accepting` in `depart`.

**Cleared rather than reworded.** There is no workspace to summarise, because nothing ran; the
STATE column already says `pending`; and `offload explain` carries the full sentence about who let
it go. An empty cell is what every other run with nothing to say shows.

The merge is safe and worth checking rather than assuming, because `RunProgress` is the subtlest
thing in this codebase. The note travels with the *position*, ranked by epoch and then by `at`
within a leg; `Supervisor::update_stats` stamps `at` on every write, and `release` has just bumped
the run's epoch, so the clearing write is the newest thing anybody has to say about the run. The
guard that stops this being a node writing in another leg's name — `leg.is_none() && stats.by ==
somebody else` — is untouched: this node *is* the leg that made the commitment.

Third time a gossiped summary has been the thing that was wrong, after the attendance line that
named a cause once it had stopped being one and `summarise_refusal` itself.

- **A field that decides one thing must not be printed as though it knew another.**

`RunState::Pending { let_go_by }` was added by ADR-0042 to separate two waits: a run a *person*
parked with `offload checkpoint` waits for that person, and a run nobody is coming back for waits
for a bid round that is actually coming. `policy::supervision` reads the field's *presence* and
returns `Place(Offering::LetGo { by })`; `explain` read its *contents* and said:

```
state       pending — waiting for a node since 28.7s ago; a728e97e let it go
            nobody holds it: host let it go rather than a person parking it, so this node
            offers it to the fleet until somebody takes it
```

`Run::release` has two documented callers, both the holder changing its mind — a drain, and a node
that cannot make the start it promised (ADR-0006) — and its doc comment said so. It has a third:
`Cluster::hand_over` gives its fencing token back when a grant is declined or never confirmed, and
that is the *arbiter* releasing a node that may never have heard of the run. ADR-0006 treats
silence as a decline in order to **act**; it is not an observation that anybody declined. So in
the swallowed case the sentence is not merely vague, it is inverted: the node named as having let
the run go is the one node that might be running it.

Found by walking the `--queue` path of session forty-four's fix, which is what put the two
sentences side by side. From one submission, seconds apart:

```
run 01a052b06670 — queued; nobody will take it yet:
  host         did not confirm: reading a frame: connection lost
...
state       pending — waiting for a node since 28.7s ago; a728e97e let it go
```

**Only visible since session forty-four.** `hand_over` has always released, but `offered` was a
local copy the round threw away, so the field reached neither the store nor the view nor a report.
`Placement::Refused { spent }` is what started keeping that record — which is right, and it
carried a claim along with the epoch it was kept for.

**Fixed in the report, not in the record.** Dropping `let_go_by` on the give-back would strand a
run: without `--queue` and without a checkpoint, `supervision` falls to `Bystanding::Unheld` and
nobody offers it again. Splitting the field into two facts is a wire change for a distinction
nothing downstream reads. What both callers can support is that the run *was assigned* to that
node and is not any more, so that is what the two lines say — `last assigned to host`, and
`it was last assigned to host, and nobody parked it`. The decision is unchanged, the two waits are
still distinguishable, and no line claims an intention this node did not witness.

## A record about a run names a command, and a command is answered by a machine

`Supervisor::recover` runs at startup, finds every run a previous daemon was holding, and writes
the reason it is failing them onto the record:

```
daemon restarted while this run was active — resumable from turn 5 with `offload resume 01a053add754`
```

That sentence is stored, and stored is the point of it: it survives the restart it describes, it
gossips, and `offload ps --all` prints it under the row as the one piece of advice a person gets
about a run that broke while nobody was looking. It is also true — the checkpoint holds the
conversation and the worktree holds the uncommitted edits, and some machine can carry on.

Since ADR-0047 the machine it is printed on may not be one of them. `offload resume` asks
`server::hosting_refusal` before it asks anything about the run, so a drained node, a revoked one,
and one whose owner said `accept = "never"` each refuse the command that record names.

Measured on one daemon, on a run interrupted by `kill -9` and recovered under
`[policy] accept = "never"`:

```
$ offload status | grep accepting
accepting   no — node is not accepting work

$ offload ps --all
01a053add754   failed              5     550       - here only  clean              walk the parser
           └─ daemon restarted while this run was active — resumable from turn 5 with `offload resume 01a053add754`

$ offload resume 01a053add754
Error: node is not accepting work
```

Two commands, one socket, seconds apart. `offload explain` had it in two places at once — the
stored sentence in its `state` line, and, for a run parked by `offload checkpoint`, its own
verdict: *"nobody holds it: it is parked with its checkpoint, and waits for `offload resume`"*.

**Not fixed by rewording the record**, which is the edit that suggests itself. `recover` has the
node's standing answer right there and could compose `…resumable, but not here`. That is the
one-field-two-facts trap that has cost this project three sessions: a permanent note about why a
run stopped would then carry a fact that changes by the hour, on a row that gossips, read back
weeks later by a node that was never asked. The record is right about the run and must go on
being only that.

So the live half travels beside the listing instead. `Response::Runs` gains `resume_refusal`,
filled from `hosting_refusal` — the resume door's own function, not a second reading of the same
three questions — and the reader puts the two together:

```
resume:    a run above says it is resumable, which is true of the run. Not of
           this node right now: node is not accepting work
```

Two things about the shape are worth keeping.

**Both halves are decided by the daemon.** The refusal is `hosting_refusal`'s; the *relevance*
gate is `supervisor::resumable_state`, split out of `refuse_unresumable` so that the report asks
the same question the door asks. A CLI deciding either for itself is the second copy of the rule
this file exists for. `resumable_state` is `Pending | Failed` — a run parked by `offload
checkpoint`, and one interrupted — and the two are structurally one predicate now, so what the
test pins is the *content*: which of the eight states a run can be found in are the two.

**The gate is per row, not per listing.** `RunSummary::resumable` rather than one flag beside the
runs, because `offload ps` hides finished runs unless asked. A node-level gate prints a footnote
about work that is not on screen — advice pointing at nothing, on the most-read command here.

`offload explain` carries the same line, beside `recovery` rather than folded into it. They are
different answers: `recovery` is what this node does **unasked**, and a run it has left for a
person is exactly one where a person is being invited to do something the node may decline. On
the revoked walk both lines were present and neither implied the other:

```
recovery    left for a person: this node has been revoked from its fleet, so it will not start an agent for it
resume      refused here right now: this node has been revoked from its fleet — it can host nothing and submit nothing until it is enrolled again
```

## …and pairing a report with the door is how you find out the door was wrong

The footnote above, on the third of the three doors, printed this:

```
01a053b4dc3b   failed              2     220       - here only  clean              revoked walk
           └─ this node was revoked from its fleet, so it stopped running it

resume:    a run above says it is resumable, which is true of the run. Not of
           this node right now: this node is draining
```

One line apart, from one daemon, about one machine: *revoked* and *draining*. The report was the
honest one.

`Supervisor::stand_down` sets **both** latches — a revoked node is also a node that takes no work,
and ADR-0044 says so in as many words — and `hosting_refusal` asked the drain first:

```rust
ctx.supervisor
    .is_draining()
    .then(|| "this node is draining; restart offloadd to take work again".to_string())
    .or_else(|| host_runs_refusal(ctx))
```

So every door that function guards told the owner of an evicted device to restart `offloadd`,
which changes nothing at all: the fleet has thrown the device out, and the way back is `offload
join`. Measured at the door, and against the *other* door a second later:

```
$ offload resume 01a053b4dc3b
Error: this node is draining; restart offloadd to take work again

$ offload run --repo /tmp/w2/repo "x"
Error: this node has been revoked from its fleet — it can host nothing and submit nothing until it is enrolled again
```

The submission is right because it reaches `host_runs_refusal` through the mesh rather than
through here. `status` is right too, and had been from the start, with the reasoning written
beside it — *standing down sets both latches and the drain's sentence is advice that cannot work
here*. The door and the report were two orderings of three questions, and only one of them had
the argument.

**Both orderings refuse, which is why it survived.** Nothing was let through, no run was
duplicated, and every test that asserted a revoked node takes no work passed. What was wrong was
the cause — which is what ADR-0043 deleted `NodeIsDeparting` for, and what ADR-0049 gave the
handover a third sentence for, met a third time at the one door a person types at. What the
operator does next differs: wait for the drain, or enrol the device again.

`revoked_refusal` is the one place now, asked above the drain by `hosting_refusal` and by
`status`, so the line a person reads and the door they then type at cannot order these
differently. Its fallback is the deliberate part: the **latch** decides that there is an answer
and `fleet.json` supplies the sentence, but where that file cannot be read the constant does. The
first version of this fix guarded the drain clause with `!is_revoked()` and let a revoked node
whose fleet state was unreadable fall past every clause to *yes* — caught by the unit test one
minute after it was written, on a fixture with a latch and no fleet file. Unknown is not good
news at a door whose good news is a second agent in somebody's repo.

- **A remembered escalation is not the current answer.** `recovery_note` matched on
  `state.decided` and printed `left for a person: {reason}` whenever it was `Some`, with no
  question about the run. The branch beside it has always asked: with no decision yet it calls
  `decide_recovery`, whose first act is to return `None` for a run that is not `Failed` — the
  *nothing to decide — it is not failed* arm. So the same question simply was not asked on the
  branch that had a memory to print.

  Found on two daemons while measuring something else. Alpha hosted the first leg, failed it while
  it was the only node, and its drain escalated `NoCopyElsewhere`. The run then moved to bravo,
  which ran it. Alpha's `offload explain` printed, in **one output**:

  ```
  holder      fedora
              its holder is answering and its lease is current
  recovery    left for a person: this node is leaving and its conversation exists nowhere else,
              so nobody could continue it (picked up once already)
  checkpoint  turn 16, copied to fedora
  ```

  Three lines saying the run is alive, replicated and supervised, and one saying nobody could
  continue it. It never self-corrected, and it never would: remembering the decision rather than
  deleting it is the whole point of `decided` (ADR-0013), so the wrongness is durable rather than
  a window.

  The memory is right and stays — an escalation has to be answerable for. What was wrong is
  presenting it as *what is true now*. Gated on `matches!(run.state, RunState::Failed { .. })`,
  which sends a run that has been picked back up down the branch that answers honestly, keeping
  the `(picked up N times already)` history that is worth saying either way.

  **The residual, and it is a behaviour question rather than a reporting one**: the entry itself
  still survives the run leaving `Failed`, and `recover_failed_runs` skips any run whose `decided`
  is set. So a node that escalated a run, lost it to a peer, and later gets it back will not pick
  it up again — only `offload resume` clears the entry. That is arguably right (the escalation was
  a real decision about a real failure) and arguably stale (it was a decision about a failure two
  legs ago). Not changed here, because the reporting fix is safe and this one needs an argument.

- **When two clocks can end a wait, the report has to name the nearer one — and which it is.**

  ADR-0035 built the drain's reporting plane so that somebody waiting is told what they are
  waiting behind, and left a residual saying the drain's deadline does not bound the question's
  patience — both 300 seconds, so a question raised *after* the drain began outlives it. The
  residual argued the mechanism should stay (expiring a question decides somebody's tool call to
  save the drain a wait) and it is right. What it missed is that its own case had already broken
  §2: `Blocked` carried `within_ms` from `ask.left` and had no idea a drain deadline existed.

  Walked on one daemon, `drain_deadline_secs = 15`, a fake agent piping a real hook into
  `offloadd ask-hook` and a `[[sinks]]` entry so the question is actually put:

  ```
    waiting for 1 run(s) to reach a turn boundary — up to 15.0s
    run 01a07310d9b4 is stopped waiting for an answer: Bash — rm -rf /tmp/nothing
      it reaches no turn boundary until that is decided, or 4m52s from now
  ```

  The right number and the wrong one, two lines apart, with the wrong one attached to the
  actionable sentence. The drain then gave up and left the run mid-turn.

  The fix is *more* reporting, not an expiry, so it finishes ADR-0035 §2 rather than reopening §4.
  `Blocked` carries `drain_ends_in_ms` beside `within_ms`; the CLI picks the nearer and says the
  consequence, because the two deadlines end the wait differently — the question expiring decides
  it and the run is then handed over normally, the drain expiring first decides nothing. After:

  ```
      it reaches no turn boundary until that is decided — and this drain stops waiting in 15.0s,
      which is sooner
      unanswered by then, the run is left mid-turn here; the question itself stands for 4m52s
  ```

  Two details worth keeping. The choice lives in `blocked_horizon`, a pure function over two
  `Millis`, because it is the only line in the product that picks between two deadlines and
  picking wrong is silent — four branches, including *equal is not sooner* (at the same instant
  the question decides it, which is the better outcome, so it is not the one to warn about). And
  `drain_ends_in_ms` is `#[serde(default)]`: zero means an older daemon, and the CLI falls back to
  the sentence it had, because half a horizon beats none.

  **The general rule.** A wait with two possible ends is a report with two numbers. Printing one
  of them is not a partial answer, it is a confident wrong one — and the tell here was on screen
  the whole time, one line above.

- **A library that swallows an error swallows the diagnosis, and the symptom lands on somebody
  else.** The mechanism is four lines of quinn-udp, and they are right:

  ```rust
  pub fn send(&self, socket: UdpSockRef<'_>, transmit: &Transmit<'_>) -> io::Result<()> {
      match send(self, socket.0, transmit) {
          Ok(()) => Ok(()),
          Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(e),
          Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => Ok(()),
          Err(e) => { log_sendmsg_error(&self.last_send_error, e, transmit); Ok(()) }
      }
  }
  ```

  Its doc comment argues the case: UDP transmission errors are non-fatal, a protocol on top has to
  retransmit anyway, and *"logging is most likely the only thing you can do with these errors"*.
  Every word of that is true, and nothing in ADR-0059 disputes it.

  What it does not say is where the *symptom* goes. quinn believes the datagram left. The
  connection times out. `offload-cluster`'s detector reports `no answer within 500ms`, `offload
  nodes` says the peer is `dead`, and every report in the product is now a confident statement
  about a machine that was never spoken to. The one machine that could have said otherwise says
  nothing — measured, not inferred: a daemon at `RUST_LOG=info` whose every send was refused had
  **zero** log lines mentioning it, over minutes.

  That is what made session sixty-six expensive. A macOS 26 laptop would not mesh with a Linux
  one; the fault was Local Network access, which is a click at that machine's console; and it
  took three sessions because every piece of evidence the product produced pointed at the peer.
  `examples/udp_probe.rs` exists only because there was no way to ask the question from inside.

  **The fix is the smallest one that separates the two facts.** `QuicTransport` binds its own
  socket and gives it to `Endpoint::new_with_abstract_socket`; `sends::WatchedSocket` is a copy of
  quinn's private tokio socket with one line changed — `try_send` where quinn calls `send` — and
  then answers what `send` would have, arm for arm. `WouldBlock` is handed back so quinn waits for
  writability; `EMSGSIZE` is ignored, because that is quinn discovering the path MTU and counting
  it would make the number noise on a VPN; everything else is counted against the destination and
  answered `Ok(())`. Behaviour is unchanged by construction, which is what makes it not a
  decision.

  **The measurement, both ways, in one walk on one daemon.** A rootless muzzle needs no firewall
  and no second machine: a UDP socket with `SO_BROADCAST` unset — quinn never sets it — is refused
  `EACCES` for every datagram addressed to the broadcast address, and nothing leaves the host. One
  daemon, `mdns = false`, `seeds = ["255.255.255.255:7433"]`:

  ```
  accepting   no — cpu at 78%: 100% of the budget is free, the machine is not
  records     0
  sends       9 refused by this machine's kernel — that is not the peer's silence.
              └─ 255.255.255.255:7433  ×9  ·  Permission denied (os error 13)
  fleet       989ebd38  ·  1 member(s) met, 1 approver(s)
  ```

  Sixteen twenty seconds later, as the seed is re-dialled. The control is the same walk with a
  real peer: two daemons meshed on loopback, gossiping for fifteen seconds, **neither** printing a
  `sends` line; then bravo `kill -9`'d, three `no answer` lines in alpha's log, `dead ~2` in
  `offload nodes`, and still no `sends` line. Two failures that had one symptom, told apart.

  And in the tests, the same muzzle without a daemon: `a_kernel_that_refuses_every_datagram_is_
  counted_rather_than_blamed_on_the_peer` dials `255.255.255.255:9` over the real transport, and
  with `send` restored in `try_send`'s place it fails on *the kernel refused every datagram and
  nothing counted one*, which is the pre-fix behaviour exactly.

  **The general rule is about attribution, not about UDP.** When a layer below you discards an
  error, the report above you is not missing a detail — it is confidently blaming the wrong
  machine, and it will keep doing so for as many sessions as it takes somebody to reach for
  `strace`. The counterpart matters as much: the temptation with a newly visible error is to
  *act* on it, and tearing a connection down or marking the peer unreachable from this side would
  put a second liveness opinion beside the failure detector, computed from a different input.
  That is this file's own rule in its most expensive form. A count and a line; the detector still
  decides.

- **A `bool` over a three-valued fact is right twice and confidently wrong once.**
  `ClusterView::steward_of` is the successor rule, and it has three answers:

  ```rust
  pub fn steward_of(&self, home: NodeId) -> Option<NodeId> {
      match self.node(&home) {
          Some(node) if !node.status.is_gone() => Some(home),
          Some(_) => self.alive().map(|n| n.id).min(),
          None => None,
      }
  }
  ```

  The third arm is the one with a meaning of its own: **this node has never met the home**, so it
  picks no successor. That is the safe answer and it stays — a daemon that has just started and
  gossiped with nobody must not begin firing the whole fleet's schedules, which is *unknown is not
  good news* in its ordinary form. It is reached by an entirely ordinary event: a laptop reboots
  while the desktop is off.

  `ScheduleReport::mine` was `steward_of(schedule.home) == Some(ctx.node_id)`, which is false for
  arms two and three alike, and the CLI's `else` branch said `fired elsewhere`. So a schedule that
  **nothing anywhere was firing** was reported as another machine's job, with a countdown to the
  next tick beneath it.

  **Measured on two daemons.** alpha homes a one-minute schedule; bravo learns it by gossip; alpha
  is stopped; bravo is restarted, so its view starts empty and its only seed is alpha. Before:

  ```
  554848b3406bcbf0  every 1m0s
      task tick
      home      978bc08d  ·  fired elsewhere
      next      in 23.5s  ·  nothing fired from here yet
  ```

  Nothing fired, across two whole tick periods, and `grep -c 554848 bravo.log` was 0 — the pass is
  silent about a schedule it is not steward of, correctly. **The tell was one line above the lie**:
  the `home` column had already fallen back to `978bc08d`, a bare node id, because
  `view.node(&home)` returned `None` — the same absence, told honestly, immediately above a line
  that contradicted it. After:

  ```
      home      978bc08d  ·  nothing here can see its home
      next      nothing fires it: no node here can see its home, so the
                successor rule picks nobody. It starts again on its own when
                that device comes back — and if it is gone for good,
                `offload unschedule` from any node, then make a new one here.
  ```

  Both halves of that sentence are measured rather than asserted. Alpha was restarted, bravo re-met
  it on the seed backoff (30s, 40s, 50s, 70s — so convergence after a home returns can take over a
  minute), the row went back to `fired by alpha`, and the schedule fired again. And `offload
  unschedule` from a node that is *not* the home works, which is the other amendment ADR-0056 took
  in the same walk.

  `Elsewhere` names the node now, which is a second small thing the `bool` was hiding: the steward
  is not always the home — arm two hands the tick to the lowest-id node that is alive — so
  "elsewhere" was sending somebody to the wrong machine's logs. The name costs one lookup the
  report was already doing for the `home` column.

  **The general rule.** When a report is built from `Option<T> == Some(x)`, look at what the `None`
  means. If it means something of its own, the report has three states and a `bool` will render the
  third as whichever of the other two the `else` happened to be — silently, in the direction of
  confidence. `steward_of` had **no tests at all** before this; it has three now, one per arm, and
  the third is named after the report it broke.

- **One name, two namespaces — and the report only knew about one of them.** ADR-0057 gave a rule
  a second way to be fired, and both names go in the same column:

  ```rust
  // `rules.service` holds the name a rule is bound to in *either* namespace, and a trigger whose
  // service is called `failed` is a legal thing to nominate — so a line arriving must not fire a
  // rule that is waiting for a notice, and a notice must not fire one waiting for a trigger.
  if spec.fired_by != expect {
      return Ok(());
  }
  ```

  That guard is in `fire_one`, and it is correct — this walk started as a hypothesis that it was
  *missing*, and measuring is what showed the hypothesis was wrong. `Service::Other` accepts any
  non-empty string, so `service = "failed"` in a `[[triggers]]` block parses, stores and displays
  as exactly the string `write_rule` stores for `offload when failed --on-notice`; the collision is
  real and the firing path is fenced against it.

  What was not fenced was `report_triggers`, which counted `rule.service == state.service` and
  nothing else. So the report credited a trigger with a rule it cannot fire.

  **Measured on one daemon, cluster off.** A `[[triggers]]` block `id = "buildwatch", service =
  "failed"` running a script that echoes a line every six seconds, a `[[tasks]]` entry to fire, and
  `offload when failed --on-notice --task page`. Before, after seven events and nothing fired:

  ```
  TRIGGER        SERVICE       EVENTS  RULES  RESTARTS  STATE
  buildwatch     failed             7      1         0  watching
  ```

  A minute earlier, before the rule existed, the same trigger had reported the honest thing —
  `0` and `watching — but no rule is bound to it`, whose own comment in the source calls it *"the
  commonest reason 'my trigger does not work' turns out to be true and uninteresting, so it is said
  before anything subtler"*. A notice-bound rule with a colliding name silently takes that sentence
  away. After:

  ```
  buildwatch     failed             4      0         0  watching — but no rule is bound to it
  ```

  **And the control, in the same walk**: `offload when failed --task page` — the *trigger* form of
  the identical name — brings it back to `RULES 1` / `watching`, fires on the next line, and
  `offload rules` then lists two rules called `failed` with `FIRED 1` and `FIRED 0`, told apart by
  the `└─` line each carries. So the filter narrowed the question rather than the answer.

  **The general rule.** When a column starts holding two namespaces, every reader of that column is
  a place the old assumption survives — and the reader that *decides* gets fixed first, because it
  is the one somebody writes a test for. The readers that only *report* are found later, by
  somebody typing the command. `offload triggers` had never appeared in any walk record in this
  repository before this one, which is how it kept the assumption for a whole ADR.

- **A report that names another line of its own output has to check the line is there.**

  **How it was found.** `offload deny` had two mentions across `DEMO.md`, `HANDOFF.md` and
  `sessions.md` — near the top of the loop in `DEMO.md` that ranks commands by how often anything
  has typed them. The walk was ADR-0017 end to end on two daemons: one clean turn from a fake
  agent, then a line piped into `offloadd ask-hook`, then `offload deny`. **The mechanism came out
  sound in every case** — the hook got `{"permissionDecision":"deny"}`, the log recorded
  `-> denied (an operator)`, the registry cleared, and the same command typed at **bravo** for a
  run held by **alpha** was forwarded and answered in one go (`denied (an operator on bravo)`),
  which is the whole of what ADR-0017 promises. What did not survive the walk was the screen
  around it.

  **The mechanism.** `offload explain` ends with one of three sentences chosen from the canvass.
  The third fires when every node refused *and* somebody holds the run, and it replaced an older
  line that said "No node would take it right now" — exactly backwards for a run already placed,
  because the canvass asks who would **take** the run and the holder therefore answers "already
  holds this run". The replacement said:

  ```
  It is taken, not unplaced — `held back` above is why it has not begun.
  ```

  `held_back` is filled in by `server.rs` behind
  `matches!(run.state, RunState::Assigned { .. }) && run.holder() == Some(node_id)` — the gate that
  would start the run, asked of this machine's occupancy and its own account's rate limit. For
  every other state it is `None` and the `held back` line is never printed. So on a `running` run
  the closing sentence pointed at nothing and asserted the opposite of the `state` line eight rows
  above it.

  **The measurement.** One daemon, default config, a run submitted `--ask --permission ask`, the
  fake agent blocked mid-tool-call:

  ```
  state       running — started 9.0s ago; lease has 52.0s left
  attendance  unattended — nobody is streaming it
  holder      alpha
  ...
  It is taken, not unplaced — `held back` above is why it has not begun.
  ```

  `offload asks` at the same instant: `01a08eae7ef6 Bash 17.3s 4m42s here rm -rf /important`.

  **The fix and its control.** `taken_not_unplaced(held_back, waiting)` — a function for
  `blocked_horizon`'s reason, one step worse, because this one names another line of its own output
  and nothing checked that the line was there. All three arms were then walked live rather than
  argued: `assigned` under `max_concurrent_runs = 1` still prints `held back at capacity: 1/1 runs`
  and the original sentence; a `running` run with a question prints the `waiting` form; the same
  run *after* the deny, and the same run explained from **bravo**, both fall to
  `the `state` line above is where it has got to`. The unit test asserts the two things the old
  line got wrong — it must not say `held back` and must not say `has not begun` — and fails on the
  pre-fix behaviour with the first arm forced (`if true || held_back`).

  **The general rule.** A sentence written against the one state that was on screen at the time
  will be rendered in every state that reaches its branch. Where a line of output refers to another
  line of output, the reference is a claim about the *rendering*, and it is the one kind of claim
  no type checks.

- **…and the state it could not describe was the one it is most typed for.**

  **The mechanism.** A run stopped mid-tool-call is `Running`, holds its lease, renews by
  heartbeat, and does nothing — for up to `approval_patience`, five minutes by default. It is the
  only way a run is `running` and making no progress by design. `offload explain`, whose whole
  purpose is "why is this not happening", had no field for it: `watchers`, `held_back`, `recovery`
  and `resume_refusal` are all observed on the holder and passed in through `Observed`, and the
  pending question — sitting in the same process, in the registry `offload asks` reads — was not.

  **The fix.** `Observed::waiting`, filled in `explain_run` from `ctx.supervisor.asks()` filtered
  to this run, and only when `run.holder() == Some(ctx.node_id)`. From the registry rather than
  from the log deliberately: the log says a question was *asked*, and whether it is still waiting
  is exactly what the registry knows and the log does not — a log-derived answer would be a second
  copy of the rule, stale by one event. Empty on a non-holder for `held_back`'s reason carried one
  step further; `offload asks` is the report that canvasses the fleet, and it already does.

  ```
  waiting     stopped for an answer: Bash — rm -rf /important  ·  4m53s left
              offload approve 01a08eb33066 toolu_walk_1   (or `offload deny`)
  ```

- **`pending?` is two arms where there are three states, and the third is where the command does
  the most.**

  **How it was found.** `offload priority` had two mentions across `DEMO.md`, `HANDOFF.md` and
  `sessions.md` — the loop again, one session after `offload deny`. Its own doc comment carries
  four claims worth measuring: *"priority decides who yields when two runs want the same machine
  at the same moment, and nothing else. A low-priority run on an idle fleet starts immediately, a
  high-priority one preempts nothing, and a run that has been waiting long enough outranks any
  priority by itself — urgency leads, and this breaks its ties."* All four are true. The note the
  command prints while saying them is not.

  **The mechanism.** `Supervisor::edit_spec` chose its note from `(edit, pending)` where
  `pending = matches!(run.state, RunState::Pending { .. })`. But a run this node has **accepted
  and not started** is `Assigned`, not `Pending` — `offload ps` renders it `waiting for a slot`,
  and it is the only state in which an order exists at all (`held_but_not_started`, sorted by
  `urgency_order`). So the state where both edits decide something got the arm written for a run
  with an agent already going.

  **The measurement.** One daemon, `max_concurrent_runs = 1`, three runs submitted a second
  apart. A is running; B (older) and C (newer) are `assigned — waiting for a slot`.

  *Priority.* `offload priority C 10` and `offload priority B -5`, each answered:

  ```
  it is already under way, and nothing is preempted for it: this decides only
  which run goes first the next time something has to choose
  ```

  B then started **first** at priority −5 and C second at priority 10 — correct, because slack
  leads and with no stated deadline slack is negative age, so two runs submitted a second apart
  never tie and priority is never consulted. The command had done nothing, and the note read as
  though it had done something later.

  *Deadline, the sharper one.* Same staging, control measured first: with no edit, B (older) ran
  before C. Then `offload deadline B 1h`:

  ```
  run 01a08edea6a2 — 59m59s left
    it is already under way — nothing speeds an agent up, so this changes only how
    long the fleet waits for it if something goes wrong
  ```

  C started **before** B. The edit had reversed the queue — an unstated deadline means *as soon as
  you can* and sorts ahead of any stated time — and the note said it changed nothing observable.

  **The fix and its control.** A third arm, `queue_note(edit, at, of)`, where the position is read
  from `held_but_not_started()` **after** the edit: the function that decides the order, not a
  second reading of `urgency_order`. Re-walked:

  ```
  === priority on C, which is 2nd in the queue ===
    it has not started yet: it is now 2nd of the 2 waiting for a slot here — urgency
    leads and priority only breaks its ties, so this moves it past nothing that is
    more overdue
  === deadline on B — the edit that actually moves it ===
    it has not started yet: it is now 2nd of the 2 waiting for a slot here — the ones
    ahead are due sooner, and a run with no stated deadline is due as soon as it can
    be, which is sooner than any time you can name
  ```

  and the prediction held: C started first, B second. The control is the same two commands against
  a run that genuinely is running, where both original sentences print unchanged. The unit test
  asserts no arm may say `under way` and every arm must say `has not started yet`; it fails on the
  pre-fix behaviour with `queue_note` returning the running text.

  **The general rule.** A boolean derived from one state name is a claim that every other state is
  the same. `Pending` and `Assigned`-unstarted differ in who holds the run and in nothing an
  operator can see, which is exactly why one stood in for the other — and the states an operator
  cannot tell apart are the ones a report must.

- **A field an operator can set has to be readable back somewhere.**

  `priority` appeared in no report in the product. `offload priority` echoed the value it had just
  written (`run 01a08… — priority 10`) and nothing would say it again: not `offload ps`, not
  `offload explain`, not `offload logs`. The gap is sharpest on `explain`, which already prints
  `due` — the slack that *leads* the same ordering — and `held back`, which is printed in exactly
  the state the pair is ordering. Half of a two-part decision was visible and the half an operator
  had just set by hand was not.

  It now prints under `due`, with what it does, and only when somebody set it:

  ```
  due         as soon as a node can take it — no deadline given, waiting 9.2s
  demand      normal
  priority    10 — breaks ties in `due`, and nothing else (ADR-0013)
  ```

  Omitted at `0` deliberately: that is the default on every run in the fleet, and a line saying so
  everywhere is noise that would bury the case this exists for. The same reasoning as `demand`'s
  emptiness check one line above it.

- **`offload unschedule` on a schedule already removed said `removed <id>`, again.**

  Exit 0, the full paragraph about the tombstone travelling, and nothing written or published:

  ```
  $ offload unschedule a2950f0f
  removed a2950f0fb24bc99a
          the removal is what the fleet is told, so it reaches nodes that
          are away — the record stays behind to carry it
  $ offload unschedule a2950f0f          # identical, and it did nothing
  removed a2950f0fb24bc99a
          the removal is what the fleet is told, so it reaches nodes that
          are away — the record stays behind to carry it
  ```

  `remove_schedule` returned a `bool` meaning *there is such a schedule*, and answered `true` both
  when it wrote a tombstone and when it found one — so the daemon republished the schedule set to
  the cluster on a no-op and the CLI described an act that had not happened. `Removal::{Done,
  Already(Millis), Missing}` now, and `Response::Unscheduled` carries `already` (control socket,
  `#[serde(default)]`, no wire bump — the CLI and daemon ship together). Still exit 0 and still
  not an error: the schedule is gone, which is what was typed, and failing a retry of an
  idempotent removal is its own wrong answer. The instant it reports is the **first** removal's,
  because `Schedule::merge` keeps the earlier tombstone and that is the one the fleet agrees on.

  `offload unwatch` got this right by accident: a rule row is `DELETE`d, so a second call finds
  nothing and refuses — measured, `no rule matching \`fcda7498\``.

- **…and the `home` line of a removed schedule still said `fired from here right now`.**

  Directly above the line saying it fires nothing:

  ```
  a2950f0fb24bc99a  every 1m0s
      home      alpha  ·  fired from here right now
      removed   it fires nothing; the record is the removal, and it gossips
  ```

  `steward_of` is asked about a tombstone like any other row and answers `Here`, because the home
  is alive and the successor rule has nothing to do. The `removed` branch below it was added in
  session sixty-nine and is careful — it replaces the countdown, which would otherwise promise a
  tick that is not coming — but it is an `if` on the *next* line and the `home` line above it had
  already been printed from the steward alone. Same defect, same report, one field over, and
  reached the second time through `removed` rather than through the steward.

  The steward clause is now `steward_note(removed, steward)`, extracted from the row precisely so
  the one rule in it is a test rather than a hope: a tombstone gets no clause at all, whichever of
  the three the steward came out as. Whose it was still prints, because that is a fact about the
  row and is what `offload unschedule --state-dir` on another node needs.

- **An absent fact must not render as a gap.**

  `offload logs` printed its agent line as one format string over two fields:

  ```rust
  println!("agent       claude-code {version}, model {model}");
  ```

  Both come from the agent's `system`/`init` line, which `offload_agent::event` reads **leniently**
  because it is somebody else's format — so either can be absent, and an agent that reported
  neither produced:

  ```
  agent       claude-code , model
  ```

  Two empty slots where the facts should be. The failure is not that a fact is missing; it is that
  the *shape* of the line is broken, so a reader concludes the report is faulty and goes to look at
  the renderer rather than at the agent. And it is the exact string `LogKind::TaskStarted` was
  split into its own variant to avoid — *"a log line claiming an agent for a run that has none,
  with two empty fields where the claim should have been"* — met from the agent side, where the run
  really does have an agent and only the details are absent.

  Named rather than omitted:

  ```
  agent       claude-code (version not reported), model not reported
  agent       claude-code 2.1.268, model claude-opus-5
  ```

  Omitting would have been tidier and wrong: *which model did this run spend on* is a question
  asked of this exact line, and a line that silently drops the answer is indistinguishable from one
  where nobody asked.

  Found by a fake agent during the ADR-0061 walks and left alone at the time as a fixture artifact,
  which was the wrong call by a small margin — a real `claude-code` reports both today, and the
  leniency in `event.rs` exists precisely because that is not a promise anybody made. Walked
  afterwards in both arms, with a fake agent that reports nothing and one that reports both.

  The mechanical part worth copying: the wording moved out of `print_event`'s `println!` into
  `agent_line`, a pure function. `print_event` writes to stdout and cannot be asserted on, so
  before the extraction **neither arm was checkable** — which is why a rendering bug lived in a
  heavily-tested crate. When a report's wording has a decision in it, the decision wants to be a
  function that returns a `String`.

- **A new tier of work needs a line on the screen its owner reads, and this one had none at all.**
  Staged with four broken nominations on one daemon — a `[[sinks]]`, a `[[triggers]]`, a
  `[[resources]]` and a `[[tasks]]` block each naming a program that is not there — and then every
  report typed:

  ```
  $ offload sinks
  phone   push   0 0 0   unusable: /tmp/…/push-that-is-missing.sh: not found on this device
  $ offload triggers
  ticker  tick   0 0 2   down, retrying: /tmp/…/ticker-that-is-missing.sh: not found on this device
  $ offload status
  resource    other:helper (helper, read, /tmp/…) — NOT usable, its program was not found
  ```

  Three of the four say it plainly, one command away. The fourth appears in none of them. The only
  places a broken `[[tasks]]` entry existed at all were one `WARN` at daemon startup and
  `offload probe --config`'s clause, which was printing the owner's label (see the probing file).

  That the missing one is the **task** is the part worth carrying: it is the only one of the four
  that is a *run*, so it is the only one a bid round places work on and the only one whose failure
  costs an epoch, a log, a notification and a retry budget. `NodeStatus::tasks` mirrors
  `NodeStatus::resources` exactly — a `#[serde(default)]` control-socket field, the CLI and daemon
  shipping together — and `usable` is read from the capability rather than measured again, so the
  line and the bid round cannot disagree.

  The method is the finding: the absent report was found by reading the **other three**, not by
  reading the code. A missing line is invisible on its own screen and obvious beside its siblings.

- **…and a refusal that names a command has to be pointed at a command that answers.** Both
  refusals this session added end *"`offload status` says which"* / *"`offload status` lists what
  this node can"*, and those were only true because the same change added the line. Session
  seventy-nine found the same class the other way round — a fix whose `Leg::Here` arm named
  `offload audit` for the one case that log is empty in — so the rule is cheap and was applied
  here while writing rather than after: type the command your sentence names, in the state your
  sentence is about.

- **An attendance line is about a *person watching*, and it claimed a run was running.**
  `explain::attendance_now` matched `(None, Some(holder)) if holder != view.local` and answered
  *"only bravo can tell; it is running there"* — for **every** state that has a holder. Measured
  twice in one walk. A run held and not started:

  ```
  state       assigned — accepted but not started; lease has 54.0s left
  attendance  only bravo can tell; it is running there
  ```

  and a run whose holder had been `kill -9`'d thirty seconds earlier:

  ```
  state       orphaned — its holder has been out of contact 29.6s
  attendance  only bravo can tell; it is running there
  ```

  The second is the worse one, and `Run::holder`'s own doc comment says why: for `Orphaned` it
  answers `Some(last)` — *who was holding it* — which the fencing file already records as the
  wrong basis for a claim about now. The arm asks `run.state` now and has three answers:
  *nothing is streaming it: bravo has taken it and not started it*, *only bravo could tell, and it
  is out of contact*, and the original for a run that really is running.

- **`it cancelled` is the run doing the cancelling.** `not_canvassed` was `format!("it {}",
  run.state.name())`, and the CLI prints it as `nobody was asked to take it: {reason}`. `name()` is
  an adjective — `completed`, `failed`, `cancelled` — and two of the three read as verbs without a
  copula, so a cancelled run was described as having cancelled something. `it is cancelled`. Worth
  recording only because of how it was found: reading the whole screen after a cancel, out loud.

## Terminal is three outcomes, and the one that reopens was getting the other two's sentence

`offload_core::supervise` is the drop-off loop as a pure function, and it answers
`Supervision::Bystander(Bystanding::Terminal)` for `Completed | Failed | Cancelled`. That is
correct and is not the defect: the loop is about holders going quiet, and it has nothing left to do
with a run that has stopped. What `offload explain` did with the answer was render one sentence for
all three — *"it is over; nothing is supervising it"* — and a `Failed` run is the one terminal state
ADR-0013 picks back up. Something **is** supervising it: the recovery tick, whose answer this same
command was printing four lines further down from `decide_recovery`.

Measured on two daemons with a fake agent exiting non-zero, on the node that failed the run:

```
arbiter     alpha (this node)
            it is over; nothing is supervising it
recovery    picking it up again in 29.0s (try 1)
```

Twice, once for an agent run and once for a task, so the tier is not what produced it — and watched
through, because a sentence about the future is only wrong if the future disagrees: the run was
`running` again on alpha twenty-nine seconds later.

Found by making `offload explain` the subject of a walk and reading whole screens rather than the
line under test, which is the same instrument that found `held_back` naming a line that was never
printed. Neither half is wrong alone; the defect exists only with both lines in front of you.

The `Failed` arm now says what that loop decided and then names **a machine**: *"this loop is done
with it; whether a failed run comes back is decided on the node it failed on"*. A machine rather
than a line, deliberately. `recovery` is built from `Supervisor::recovery_state`, an in-memory map
on the node the run failed on, so it is printed nowhere else — and a sentence saying *see `recovery`
below* would be this file's own rule about naming another line of your own output, arriving from the
other direction. `Completed` and `Cancelled` keep the old words, which were always true of them, and
the test asserts both directions so that a fix which hedges about every terminal run fails.

## A pairing between a run and a node has to be right about the run first

Two reports pair a fact about the *run* with a fact about the *node*. `offload ps` carries
`RunSummary::resumable` per row and `Response::Runs::resume_refusal` for the listing;
`offload explain` carries the same two as one `resume` line. Both gated relevance on
`supervisor::resumable_state` — `Pending | Failed` — which is the **door's** predicate, shared with
`refuse_unresumable` precisely so that a listing cannot offer what the door would refuse. It is
tier-blind, and correctly so: `Supervisor::restart_task` admits a run by the same states, because
what a run has to *be* for either door to reopen it is a property of the run, and two copies of that
match would be two chances to disagree about it.

The reports needed a different question. ADR-0058 gives a failed task its own verb and its own door:
it is restarted from its spec, never resumed, because there is no conversation to continue. So
`offload resume` refuses every task on every node, for ever, and no node-level fact can change that.

The staging that shows it is a fleet of one with a `[[tasks]]` entry whose program exits non-zero,
and then `offload drain`, which is the cheapest way to give a node a refusal to state:

```
resume:    a run above says it is resumable, which is true of the run. Not of
           this node right now: this node is draining; restart offloadd to take work again
```

— over a listing whose only failed run is a task. The two-daemon form is the same sentence one
cause over, from `offload explain` on a node that was never granted `host-runs`: *"resume refused
here right now: this node has not been granted host-runs — run `offload grant host-runs`"*. Both
name something the reader can go and change. On the node that **had** the grant and was not
draining, `offload resume` on that same run answered *"it has no checkpoint — there is no
conversation to continue"*: a node-level, temporary-sounding refusal standing in front of a
run-level, permanent one. The clause *"which is true of the run"* is what makes it a lie rather than
a caveat, and it is the half nobody would have thought to check.

Found by typing the command the sentence named, in the state the sentence was about — session
seventy-nine's rule, applied to a tier it was not written for. `resumable_by_hand` is the reports'
predicate now: the door's states **and** the tier. The door and `restart_task` keep the tier-blind
one. Walked against a build with the two call sites reverted, on the same state directory, with the
control in the same listing — a failed *agent* run, which keeps its footnote.

## A report's silence borrowed its excuse from the neighbour whose silence had a different cause

`offload explain` computes two of its lines only on the run's holder. `held_back` is arithmetic —
this machine's occupancy and its own account's rate limit — and no other node's copy of the record
could produce it. `waiting`, the ADR-0017 block, got the same treatment with the reasoning written
down beside it: *"for `held_back`'s reason carried one step further: a blocked process exists on
exactly one machine, so another node's copy of the record cannot answer and must not appear to."*

That step does not hold. A blocked process does exist on one machine — and a *question* is a thing
that machine can be **asked** for. `offload asks` has canvassed the fleet for exactly this since
ADR-0017 landed, and that ADR's own text gives its reason as *"`offload explain`'s reason"*: the
precedent it cites is the one report that was not doing it.

Measured on two daemons, with a fake agent piping a `PreToolUse` payload into `offloadd ask-hook` at
turn 2. On the holder `offload explain` is complete — the tool, the clock, and both commands with
the run id whole. On the peer, the entire report about a run that had been stopped for five minutes
was:

```
state       running — started 9.0s ago; lease has 51.7s left
attendance  only alpha can tell; it is running there
```

`offload asks` **on that same socket** printed the tool, `WHERE alpha`, `4m51s` and the
`offload approve` line — and typing it there unblocked the desktop's agent, because an answer
travels. The person reading the silent screen was one command away from fixing what the screen would
not tell them was wrong.

The fix is one canvass in the explain handler when this node is not the holder, bounded by the same
`bid_window_ms` as the opinions round above it and skipped for a terminal run for that round's
reason — there is no live question on a run that has stopped.

Fixing it uncovered the other half. With the block visible off the holder, `attendance_now`'s
held-elsewhere arm put *"it is running there"* one line above *"stopped for an answer"*. That arm
has now over-claimed three times — `Assigned`, `Orphaned`, and this — and the first two are readable
from `run.state` where this one is not, which is why it could only appear once the fact arrived. The
`state` line still says `running`, which is the honest word for the process; an attendance line is
about *who is watching*, and its justification clause has no business restating a claim the line
below it qualifies.

Found by reading one run from two machines. Every vantage-dependent line in that report is a
candidate, and the question to ask of each is not *can this node see it* but *can this node ask*.
- **A dial to a dead peer does not fail, it times out.** Session ninety, `offload continue` on alpha
  for a run whose last leg ran on bravo. With bravo killed by `pkill` (SIGTERM) the answer was
  immediate — a draining daemon closes its connections with `shutting down`. With `kill -9` and
  `offload nodes` already printing `bravo dead ~3`, the same command took **30.0 s** and ended
  *"did not answer (no route to 2a3b5d5d: 127.0.0.1:17432: timed out)"*: QUIC over UDP has no
  refusal for a process that is simply gone, so the dial waits out its window, and the window was
  the continuation's 30 s because building a base is a capture rather than a lookup. The fix asks
  the view first — `Dead` or `Departed` refuses at once with *"bravo, which ran its last leg, is
  dead, and its workspace is there and nowhere else"* (8 ms, measured). `Suspect` and `Draining`
  are still dialled, for `fetch_blob`'s reason: both are usually up. Worth checking at every other
  forward (`cancel_at`, `answer_at`, `checkpoint_at`), which use the shorter bid window and were
  not measured here.

- **A canvass that folds every transport error into `no answer` blames the peer for this node's
  reach.**

  Session ninety-one, walking the two `explain` vantages session eighty-six left: a run held by a
  *third* node, and a node that has never met the run's home. Three daemons on loopback — alpha
  founding and submitting (agent `/bin/false`), charlie the only node with a fake agent, bravo
  enrolled without `host-runs`; bravo and charlie each seeded with alpha only. alpha's canvass had
  every node's answer; bravo's said `charlie  no answer` and charlie's said `bravo  no answer`.
  Neither was silent. A loopback bind announces no address, so a node knows only the addresses of
  its seeds and of whoever dialled it, and bravo and charlie had never met directly — each showed
  the other `alive` purely through alpha's gossip. `QuicTransport::connect` returned
  `Unreachable { reason: "no known address — not discovered yet" }`; `ask_one` turned every `Err`
  into `Ok(None)`, which is `Verdict::Silent`, which prints `no answer`.

  Fixed as a fourth verdict rather than a reason on `Silent`, because `place`'s round and
  `server.rs`'s preference sentence both match on it and each wants to say something different.
  Re-walked on the same staging: `charlie  not reachable from this node: no known address — not
  discovered yet`. Over QUIC a dial to a dead peer with a *known* address is also `Unreachable`
  (`timed out`), which the variant's doc says, and the in-memory transport's partition is
  `Unreachable("partitioned")` — which is what the mesh test that pinned `no answer` now reads.

- **…and `explain`'s `due` line said `waiting 46.0s` about a run four lines under `state
  running`.**

  Same walk, every vantage including the holder's. `due` for a no-deadline run printed `as soon as
  a node can take it — no deadline given, waiting {now - created_at}` whatever the state. Session
  eighty-something fixed the finished half (`it ended …`); the held half was left saying a node
  should take something a node had taken. Branches on `run.holder()` now.

- **`nobody arbitrates it here` is not a first-second-of-gossip state.**

  Same staging, then `kill -9` alpha (the home), wait for bravo and charlie to mark it dead, and
  restart bravo. Its view holds only itself — its one seed is dead — so `steward_of(alpha)` is
  `None` and `explain` printed `arbiter undecided here / nobody arbitrates it here: this node has
  never met its home node bd5f5c39`, for as long as it ran. The arm's comment said that was *what
  a node says in its first second of gossip*. Restarted again with charlie added to its seeds: it
  met charlie, learned alpha (dead) from charlie's gossip, the successor rule picked bravo as the
  lowest live id, and the screen read `arbiter bravo (this node)`. So it heals through any peer
  that remembers the home, which is what the sentence now promises, and nothing else.

  The staging has a trap of its own, in `docs/DEMO.md`: two joiners seeded only at the founder, on
  loopback, are **partitioned** the moment the founder dies, so bravo orphaned charlie's run while
  charlie carried on — correct behaviour for a partition, and not what the walk was about.

- **…and `offload checkpoint` and `offload approve` still dialled, then said the holder *is
  running* the run.**

  Session ninety-one, the three forwards the `continue` entry above named and did not measure.
  Two daemons (alpha arbitrating, charlie with a fake agent), a run on charlie, `kill -9` charlie
  and its agent, wait until `offload nodes` on alpha says `dead` (~8 s) — by then `offload ps` says
  `orphaned`. Then from alpha:

  ```
  offload checkpoint  2.02s  charlie, which is running this run, did not answer (connection to 749c4d1b closed: no answer within 2000ms)
  offload approve     2.01s  charlie, which is running this agent, did not answer (…)
  offload cancel      0.00s  charlie holds run … and is out of contact, so a cancel cannot reach its agent yet — …
  ```

  `cancel` had an `Orphaned` check of its own; the other two had none, and `continue` had a third,
  different one (the view's `Dead`/`Departed`). `holder_out_of_reach` is the union, and all three
  refusals now name the cause the same way (`… and is dead, so …`), each under 10 ms. The window
  is only `bid_window_ms` (2 s) here, so the wait was minor — the defect was the sentence, a
  present-tense *"is running"* about a machine two commands called dead. `checkpoint`'s first
  wording said *"there is no agent to stop"*; a node marked dead may be partitioned with its agent
  still going, so it says *nothing can reach its agent* instead.

*The four entries below were backfilled in session ninety-one from `docs/sessions.md` and the
commits that added each rule (`5d34733`, `32a93ef`, `73ce8e3`) — sourced, not reconstructed from
the rule text.*

- **A new tier of work makes every existing report a claim about something it has never seen.**

  Session sixty-six, third half (commit `5d34733`), the first task ever run. It printed `agent
  claude-code , model` — an `AgentStarted` line with two empty fields where the claim should have
  been — and `offload cancel` answered *"its agent was stopped"* about a shell script. Both
  sentences were written when an agent was the only thing a run could be. Adding a kind of work
  means re-reading every sentence that describes work, and running one and reading the output finds
  them faster than grep does.

- **A record written before the decision is a record of a decision that was not made.**

  Session sixty-seven (commit `32a93ef`). `build_task` called `save_run`; `build`, the agent path it
  was copied from, never has. So a task submission the round refused left a `pending` row on disk
  and in gossip, one line under `offload run`'s own *"(use `--queue` to leave it pending anyway)"*.
  Measured on one daemon; `offload explain` was the honest report of the three: *"nobody holds it,
  and it was not queued — nothing offers it to the fleet again on its own."* `place` records what it
  decides to keep, so a builder that persists has made the arbiter's decision for it.

- **…and a `Display` written with `write_str` is not a column.**

  Same commit. `kind` went from a `String` to a `WorkKind` so the CLI could compare a variant rather
  than spell `"task"` twice — and it broke the column: a `Display` written with `write_str` ignores
  the width in `{:<5}`, where `f.pad` honours it. `task` is one character shorter than `agent`, so
  every numeric column to its right sat one place left on task rows — invisible in a table with one
  row, obvious with one of each tier. There is a width test now.

- **A task's log was unreachable from the machine it was submitted from, and the failure was
  silence.**

  Session sixty-eight, phase 8's demo (commit `73ce8e3`). `offload logs` on a task printed nothing:
  exit 0, no output, on the submitting node. `server::log_source` asks `RunProgress::by` which
  machine serves a finished run's log, since the leg that wrote the numbers wrote the log; a task
  writes no numbers, so nothing stamped the leg and the answer fell through to the run's arbiter —
  here. The same silence session sixty-two removed for agent runs, back for the cheap tier. The
  control was in the same walk: the escalated agent run's log forwarded from the same node a minute
  later. `Supervisor::note_log_leg` now stamps the leg where the log starts.
