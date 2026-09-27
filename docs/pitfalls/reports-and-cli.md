# Reports and the CLI

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/reports-and-cli.md`, same order.

The rule under most of these: **a report has to come from where the decision reads.** A command
that tells somebody what the system will do, computed from a second copy of the rule, is
confidently wrong and silent.

- **A command names a run, and a run is on a machine — so it acts there or says it cannot.**
  `cancel` read this node's *process table* and reported "not running" about a run running on the
  desktop; `rm` **succeeded** and gossiped `workspace: "removed"` about another machine's disk.
  Cancel travels (wire v21), with two kinds of target: a *held* run is on its holder, an unheld one
  belongs to its `arbiter_for`. A run whose holder went quiet is refused, not forwarded into a
  timeout. `offload checkpoint` travels too (v22) and has only the first kind — a turn boundary is
  a moment in a *process*. `Removal::{Removed, NothingHere}`.
- **A run nobody holds still has a log, and it is the arbiter's.** A pending run's one message is
  the arbiter saying it will miss its deadline. Safe to follow: a non-holder reports its leg ended.
- **A log line must not be written when the state it names was refused.** `Run::cancel` correctly
  refuses a terminal run, and the log line was written anyway — so a run that had just succeeded
  had `cancelled` as the last word in its own log.
- **A log kind named after a state must not be borrowed for an operation inside it.** A failed
  *capture* logged as `LogKind::Failed` hung up every follower (taking the attendance that decides
  auto-resume) and told somebody their run had failed a minute before it finished. `CaptureFailed`.
- **A command that reports a policy has to report the policy in *force*.** `offload policy`
  computed the class default, so a phone configured `accept = "always"` was told otherwise. It
  takes `--config` and goes through `Config::work_policy`, and it *says which* policy it answers
  about.
- **A default that differs from the neighbouring command's has to be said where it is chosen.** A
  rule written months ago is where "no notification" and "no failure" look identical.
- **Run ids are UUIDv7, so their leading bytes are a clock.** Every run submitted within about a
  minute shares its first 8 hex characters. Displayed prefixes are **12** — one `const` in
  `byte_id!`, for `RunId` alone; a `NodeId` and a `BlobHash` are uniform.
- **…and a report must not claim what only a daemon could know.** `offload fleet` passed `health`
  an empty certificate list, which reads as "met nobody" — so a laptop gossiping with an approver
  was told its fleet had none. `Met::{Handshakes, NotAsked}`: a daemon's empty list is a fact, a
  CLI's is an absence of evidence.
- **`Millis`'s `Display` renders a duration**, so an absolute instant printed through it is always
  wrong (`496117104h5m`). `offload-core` has no clock and cannot format wall-clock time at all.
- **A report has to say what the decision says** — three in one sweep, one bug in three hats.
  `offload policy` computed the class default; `offload probe` printed `Battery { charging: true }`
  where the gate reads `on_mains()`, inviting somebody to think a battery floor applied; and
  `offload fleet --socket <path>` answered from `$HOME/.offload`, because `--socket` picks a
  *daemon* and membership is read from the state directory. Where a flag cannot apply, saying so
  beats ignoring it.
- **A run nobody holds got there two ways, and one fallback covered both.** `logs` falls back to
  `arbiter_for` — right for a *pending* run, and for a *finished* one the arbiter is precisely the
  machine that never had the log. Use `RunProgress::by`, the leg that wrote the position. When a
  value is `None` for two reasons, a single fallback is right for at most one of them.
- **A countdown is not a fact about a run that has stopped running.** Once a run is over the
  question changes from how long it has to whether it made it (`Run::prospect_at`).
- **`offload explain` has to answer in every state, and `assigned` was the one it could not.**
  `Supervisor::start_refusal` is the **same call** `start_held_runs` makes, so the report cannot
  disagree with the gate and a new reason arrives without being enumerated.
- **…and it said "nobody is watching" about a run it was keeping *for* the watcher.** Attendance is
  observed at the moment of failure, because a failure ends the stream — sampling later finds
  nobody every time. The escalate branch **records** its decision instead of deleting the entry:
  deleting state to stop something being repeated also stops it being answered for.
- **A header and its rows are one table, so they must not be two widths.** One `const` shared by
  both lines; two numbers that agree only because somebody remembered are two things to remember.
- **A bid describes one second, so nothing replays one.** `offload explain` re-canvasses. Silence
  stays its own verdict: "would not" and "did not answer" are different facts.
- **A verdict reads from the right function and can still describe the wrong branch.**
  `offload explain` asks the same `offload_core::supervise` the loop runs, which is the rule this
  file exists for — and its `Bystanding::Unheld` arm said *"waiting for a bid round"*, which is
  what `Supervision::Place` does and what this arm is the `else` of. Inherited from `Unheld`'s own
  doc comment, written when no round was ever held without a person and left behind when ADR-0014's
  queued runs gave those words to the sibling. Reading the decision is half the rule; the sentence
  has to be about the arm you are in. Its own guard now, and the old one asserted the wrong string.
- **A count that means two things gets one sentence, and it is right for at most one of them.**
  ADR-0035 split `finished` out of a drain's `left` for this; `later` came out of it for the same
  reason one outcome on (ADR-0041). `left` means *final* — nobody would take it — and the advice
  attached to it ("`offload ps` and `offload nodes` say which") cannot answer a run that is simply
  still mid-turn.
- **A refusal that will never lift must not be logged as one that will.** `start_held_runs` logged
  every failed start under `could not start it yet`, with a comment promising the next tick would
  try again — and a terminal run holds no lease, so it never appears in `held_but_not_started`
  again. Nothing tries anything. That line is the ordinary end of a cancel landing while the tick
  works through the run ahead of it, so it is one an operator meets: `SubmitError::Ended` is its
  own variant precisely so the *caller* can tell the two apart, the way `LostTheRun` already is.
  Its sentence for a person is deliberately unchanged from the `Refused` it replaced — the variant
  is for the caller, not for the reader — and the level drops to `info`, because a cancel that
  worked is not a warning.
- **…and a held run publishes its refusal into the worktree column, so something has to take it
  out again.** A run held under ADR-0006 has no worktree, so the column carries *why it has not
  started* instead — and that is a sentence about a moment. Left there when the run ends it becomes
  the fleet's standing answer: measured on a daemon, `offload ps` showed `cancelled` beside
  `waiting for a slot` for as long as the record existed, while `offload rm` on the same run said
  there was no checkout at all. `NEVER_STARTED` on the branch that never started — and only that
  branch, since a `Running` row reaching `cancel_run` after a restart has a checkout and its last
  summary is still true. Walked both ways in one pass: the held run came out `never started`, the
  running one kept `clean`.
- **A verdict built on the right function still guesses, if the thing that decides is not in the
  function.** `offload explain`'s verdict is `offload_core::supervise`'s, deliberately — a second
  copy of the rule disagrees quietly on the day it matters. But a drained node's handover debt is
  node-local and in memory, gossiped nowhere by design, so no pure function of the view can see it;
  the report inferred from what was left on the row, and a checkpointed `Pending` run is what
  `offload checkpoint` leaves too. So a run this node was re-offering every backoff was described
  as *"parked with its checkpoint, and waits for `offload resume`"* — sending somebody to do by
  hand the thing already in progress. Hand the fact in (`Observed`), the way `departing` already
  is. **Reading the right function is half the rule; the other half is that everything it cannot
  see has to arrive as an input.**
- **…and whether anything will try again is not a property of the error.** Classifying the tick's
  failures by variant got the terminal case right and left two behind: a run that had **moved to
  another node** still refused with `agent: cannot assign a run that is running` — a transition
  error handed to an operator — under `could not start it yet`, which nothing would. The error says
  what went wrong; the *filter* says whether the tick comes back. `Supervisor::awaits_a_slot` is
  that filter, written once, and `will_try_again` asks it about one run — sound only because a
  failed start now takes its registration back. `LostTheRun` is the sentence for the moved case,
  being the variant that already means it. The third refusal `assign` can produce, a `Pinned` run
  with an attempt spent, has three gates in front of it (`release` refuses to return one to the
  pool, `reassign` holds it with `PinnedToHolder`, a drain keeps it with `PinnedHere`) — so it gets
  a readable sentence and not a variant, until something can produce it.
- **Terminal is three outcomes, and the nicest one is not the default.** `is_terminal()` covers
  `Completed | Failed | Cancelled`, so a single sentence about "the run ended" is right for one of
  them. Measured an hour after it shipped: the pass that settles a drain's owed handovers logged
  `why="it finished here first"` for a run somebody had just **cancelled**, and again for one whose
  agent had **died** — and that line is the only record of what became of a run the drain
  *promised* to hand over. Same rule that gave `Boundary` four answers and `Drained` a fourth
  count; a state added later says its own `name()` rather than inheriting one of the three.
- **…and the better fix was to stop the decision from being blind, not to feed the report.** The
  entry above is right about the shape and settled for the smaller half: a fact handed to
  `explain` on the side is one every *other* report still cannot see, and the daemon's own log
  line said `queued run placed` about a run nobody queued. Putting the fact on the record
  (ADR-0042) put it inside `supervise`, and the decision then carries its reason —
  `Supervision::Place(Offering::{Queued, LetGo { by }})` — so the verdict, the state line and the
  log all come from one place and the side channel could be deleted. **When a report has to be
  told something the decision cannot see, ask first whether the decision should see it.** Two
  sentences that were wrong the moment `Place` grew a second way in, and neither would have been
  caught by a test of the arm they were written for.
- **A count that has been split once will need splitting again.** `Drained` is on its fourth and
  fifth numbers: `finished` came out of `left`, then `later` came out of `left`, and now `pooled`
  has come out of `later` — a run still mid-turn needs this process to reach the boundary it is
  handed over at, and a run released into the pool does not need this daemon at all. That is the
  difference between the two sentences somebody about to close a laptop is choosing between, and
  it was one number. The rule is the same one three times, so it is worth stating as a habit
  rather than a lesson: **when you teach the daemon a new way for a run to end up somewhere, look
  at every count that already covered the old ways.**
- **A note about why this node has not started a run outlives the node's intention to.**
  `take_run` writes the refusal into `RunProgress::workspace` — "waiting for a slot", "at capacity:
  1/1 runs" — and it **gossips**, so it is what the whole fleet reads in `offload ps`. Nothing
  cleared it when the commitment was handed back, so a released run sat `pending` with no holder
  under a sentence naming a wait that had ended. Measured while walking `GiveBack`: two minutes on
  an idle node reporting `runs 0/1 · accepting yes`, `offload ps` still saying `waiting for a
  slot`. Cleared in `release_unstarted` rather than in either caller — the drain and the
  late-commitment pass both let a run go — and cleared rather than reworded, because there is no
  workspace to summarise (nothing ran) and the state column already says `pending`. Same family as
  the attendance line that named a cause after it stopped being one, and the third time a
  *gossiped* summary has been the thing that was wrong.
- **A field that decides one thing must not be printed as though it knew another.** `let_go_by`
  exists to decide *offer it again* (ADR-0042), and `explain` printed it as `host let it go` —
  an intention. `Run::release` has a third caller nobody printing that had in mind: a bid round
  giving its token back when a grant is declined or **never confirmed**, which names a node that
  never held the run and, in the unconfirmed case, may be the one node running it. Measured on
  three daemons: `offload run` said `host did not confirm`, and `offload explain` on the same
  event said `a728e97e let it go`. Both lines now state the record — `last assigned to` — which
  still separates the two `Pending` waits, because that was all ADR-0042 asked of the field.
  Only visible since the round's record started being kept: the give-back had always set it, and
  the copy it was set on used to be thrown away.
- **A record about a run names a command, and a command is answered by a machine.** `recover`
  writes `failed — resumable from turn N with 'offload resume <id>'` at startup and stores it, so
  the sentence is permanent. It is right about the *run* — the conversation and the uncommitted
  edits are there — and it says nothing about the node, which since ADR-0047 may refuse that exact
  command. Measured on one daemon: `offload ps --all` printing the invitation while `offload
  resume <id>` on the same socket answered `node is not accepting work`, one command apart. **Not**
  fixed by rewording the record, which is the edit that suggests itself and is the one-field-two-
  facts trap: a permanent note about why a run stopped is the wrong home for a fact that changes
  by the hour. The live half travels beside the listing instead (`Response::Runs::resume_refusal`,
  from `hosting_refusal` — the resume door's own function) and the two are paired at the reader.
  Both halves come from the daemon: the gate is `supervisor::resumable_state`, the predicate
  `refuse_unresumable` admits a run by, and it is **per row** because `offload ps` hides finished
  runs unless asked and a node-level gate would print advice about work that is not on screen.
  `offload explain` carries the same line, beside `recovery` and not folded into it — what this
  node does *unasked* and what it will do *when asked* are two answers, and a run left for a
  person is precisely one where the person is being invited to do something the node may decline.
- **…and pairing a report with the door is how you find out the door was wrong.** The footnote
  above, on a revoked fleet-of-one, read `not of this node right now: this node is draining` one
  line under a run that had failed with `this node was revoked from its fleet`. Two sentences from
  one daemon, one line apart, disagreeing — and the report was the honest one. `stand_down` sets
  **both** latches (ADR-0044) and `hosting_refusal` asked the drain first, so every door it guards
  told an evicted device's owner to *restart `offloadd` to take work again*, which cannot work.
  `status` had the right ordering and the reasoning written beside it the whole time; the door and
  the report were two orderings of three questions and only one of them had the argument. Both
  refuse, which is why it survived — a wrong cause confidently stated is what ADR-0043 deleted
  `NodeIsDeparting` for. `revoked_refusal` is now the one place, shared by both, and it falls back
  to the constant rather than to `None` when `fleet.json` cannot be read: unknown is not good news
  at a door whose good news is a second agent in somebody's repo.
- **A remembered escalation is not the current answer.** `explain` printed `decided` unconditionally,
  so a node that escalated a failure went on saying *nobody could continue it* about a run since
  picked back up elsewhere — in the same output as *its holder is answering and its lease is
  current*. Gated on the run still being `Failed`, which is the question the undecided branch beside
  it was already asking.
- **When two clocks can end a wait, the report has to name the nearer one — and which it is.**
  `DrainStep::Blocked` carried the *question's* remaining patience and knew nothing about the
  drain's deadline, so a drain saying `waiting … up to 15.0s` was followed two lines later by
  `it reaches no turn boundary until that is decided, or 4m52s from now`. The second is the one
  somebody acts on. They are also different **events**: the question expiring decides it and the
  run is handed over normally, the drain expiring first decides nothing and leaves the run
  mid-turn with the question still open. Both numbers travel now and the CLI picks, in
  `blocked_horizon` — a function rather than three `println!`s in a match arm, because it is the
  only line in the product that chooses between two deadlines and choosing wrong is silent.
- **A new tier of work makes every existing report a claim about something it has never seen.**
  The first walk of a task printed `agent claude-code , model` — an `AgentStarted` log line with
  two empty fields where the claim should have been — and `offload cancel` answered *"its agent
  was stopped"* about a shell script. Both were written when an agent was the only thing a run
  could be, and both read as confident nonsense the moment that stopped being true.
  `LogKind::TaskStarted` and a cancel line that names what it stopped are the fix; the rule is
  that **adding a kind of work means re-reading every sentence that describes work**, and the
  cheapest way to find them is to run one and read the output rather than to grep. Still open in
  the same family: `offload ps` renders a task's service in the **prompt** column, which is
  honest and is not a substitute for a kind column (`docs/ROADMAP.md` phase 8).
- **A record written before the decision is a record of a decision that was not made.**
  `build_task` called `save_run` and `build` never has: with a cluster, `place` refuses a
  submission nobody will take and `offload run` says *"(use `--queue` to leave it pending
  anyway)"* — one line above a `pending` row that was already on disk and gossiping. Measured on
  one daemon: `no node will take this run`, then `offload ps --all` showing it, then `offload
  explain` correctly saying *"nobody holds it, and it was not queued — nothing offers it to the
  fleet again on its own"*. `place` records what it decides to keep (`take_run` for a run this
  node accepted, `record_run` for a queued one), so a builder that persists is a builder that has
  made the arbiter's decision for it.
- **…and a `Display` written with `write_str` is not a column.** The `KIND` field started as a
  `String` and became a `WorkKind` so that the CLI could compare a variant instead of spelling
  `"task"` a second time — and `{:<5}` stopped padding, because `write_str` ignores the
  formatter's width where `f.pad` honours it. `task` is one character shorter than `agent`, so
  every numeric column to its right sat one place left on task rows. Invisible in a one-row
  table and obvious with one row of each tier, which is the only way it was going to be seen:
  the type change was strictly an improvement everywhere except the one place it was printed.
  There is a width test now, because this one is mechanically checkable.
- **A task's log was unreachable from the machine it was submitted from, and the failure was
  silence.** `server::log_source` asks `RunProgress::by` which node serves a finished run's log,
  on the sound grounds that the leg which wrote the numbers wrote the log — and a task writes no
  numbers, so nothing stamped the leg and the answer fell through to the run's **arbiter**, which
  for a run submitted here is *here*. `offload logs <task>` then printed nothing at all: exit 0,
  no output, which reads as "that run produced none". Exactly the silence session sixty-two
  removed for an agent run, back for the cheap tier, because the fact it turned on was an agent's
  side effect. Measured on two daemons: the whole output on the node that ran it, nothing on the
  node that submitted it, `progress_by` NULL in both rows — and the escalated *agent* run beside
  it forwarding correctly, which is the control. `Supervisor::note_log_leg` writes the stamp where
  the log starts (so a task killed mid-output is still findable) with an empty closure on purpose:
  the write is the signature, not the numbers. **A fallback that covers two cases covers the new
  one badly** — the fall-through was documented as the pre-schema-v7 row, and a whole tier landed
  in it.
- **A library that swallows an error swallows the diagnosis, and the symptom lands on somebody
  else.** `quinn_udp::UdpSocketState::send` returns `Ok(())` for every send error but
  `WouldBlock` — correctly, since a UDP send error is non-fatal — so a machine whose kernel
  refuses every datagram believes it sent them all, and the only sentence the product had was the
  detector's `no answer within 500ms`: **the peer did not reply**. Three sessions went into a
  macOS laptop whose real fault was Local Network access, and the one piece of evidence that ever
  named it was an `EHOSTUNREACH` seen by hand under `strace`. `try_send` hands the error back;
  `sends::WatchedSocket` counts it and then answers exactly what `send` would have (ADR-0059), so
  behaviour is unchanged by construction and only the observation is new. **The rule is about
  attribution, not about UDP:** when a layer below you discards an error, the report above you is
  not merely missing a detail — it is confidently blaming the wrong machine. Measured both ways
  on one daemon: seed `255.255.255.255:7433` (refused `EACCES`, nothing leaves the host) gives
  `sends 9 refused by this machine's kernel` climbing to 16, with **nothing** in the daemon's own
  log and a healthy `offload nodes`; a peer `kill -9`'d in the same walk gives three `no answer`
  lines, `dead ~2`, and no `sends` line at all.
- **…and the counterpart is that the fix must not become a second opinion.** The temptation with
  a newly visible error is to act on it — tear the connection down, mark the peer unreachable
  from this side. That would be a second liveness mechanism beside the failure detector, deciding
  from a different input, which is this file's own rule in its most expensive form. A count and a
  line; the detector still decides.
- **A `bool` over a three-valued fact is right twice and confidently wrong once.**
  `ClusterView::steward_of` answers *this node*, *that node*, or **nobody** — the third when the
  schedule's home is not in this node's view at all, which is any daemon that restarted while the
  home was away, or a device retired for good. `ScheduleReport::mine` was
  `steward_of(home) == Some(me)`, so `offload schedules` said **"fired elsewhere"** for a schedule
  nothing anywhere was firing, and printed a countdown to the next tick under it. Measured on two
  daemons: bravo restarted while alpha was off, held a live copy of alpha's schedule, and reported
  `home 978bc08d · fired elsewhere` / `next in 23.5s` across two whole tick periods while firing
  nothing. **The honest half of the same fact was one line above** — `home` had already fallen
  back to a bare node id, because that node is not in the view either — and the two lines
  contradicted each other. `api::Steward::{Here, Elsewhere, Nobody}`, and the `Nobody` arm says
  what is true and what to do about it rather than printing arithmetic. The tell to look for: a
  report built from `Option<T> == Some(x)` where the `None` has a meaning of its own.
- **…and `Elsewhere` has to name the node, because the steward is not always the home.** A home
  that has gone hands the tick to the lowest-id node that is alive, so "elsewhere" sends somebody
  to the wrong machine's logs. It costs one lookup in the view that the report was already doing
  for the `home` column.
- **One name, two namespaces — and the report only knew about one of them.** ADR-0057 let a rule
  be bound to a **notice** instead of a trigger's service, and both names live in the same
  `rules.service` column, distinguished by `fired_by`. `fire_one` asks both questions, with the
  collision named in a comment beside it (*"a trigger whose service is called `failed` is a legal
  thing to nominate"* — `Service::Other` accepts any string). `report_triggers` asked only the
  first, so `offload triggers` credited a trigger with a rule only a notice can fire. Measured on
  one daemon with a `[[triggers]]` block named `failed` and `offload when failed --on-notice`:
  seven events, nothing fired, and the state line went from **`watching — but no rule is bound to
  it`** to plain `watching` — which is the one sentence that report exists to say, and is
  documented in its own code as the commonest reason a trigger "does not work". The control is the
  same walk with a *trigger*-bound rule of the identical name: `RULES 1`, `FIRED 1`, while the
  notice-bound one beside it stayed at 0. **When a column starts holding two namespaces, every
  reader of that column is a place the old assumption survives** — and the reader that decides
  will be fixed first, because it is the one somebody tests.
- **A report that names another line of its own output has to check the line is there.**
  `offload explain` closed a held run with *"It is taken, not unplaced — `held back` above is why
  it has not begun"*, and `held_back` is only ever computed for a run in **`assigned`**. On a
  `running` one it pointed at a line that was never printed and told somebody a run that started
  nine seconds ago had not begun — two lines of one screen contradicting each other, the cheapest
  defect there is to see. The first clause was right in every case and is why the sentence exists;
  it was the second, written against the one state that was in front of somebody at the time, that
  did not generalise. Now a tested function picks the clause from what was actually printed
  (`taken_not_unplaced`), and the fallback names `state`, which is always there.
- **…and the state it could not describe was the one it is most typed for.** A run stopped
  mid-tool-call waiting for a person (ADR-0017) is the one way a run is `running` and making no
  progress **by design**, and `offload explain` said nothing about it at all: a state line, an
  attendance line, and a canvass saying every node was busy or refused. `offload asks` one command
  away had the question, the clock and the id. Explain now carries `waiting`, read from the same
  registry on the holder so the two cannot disagree — and empty on any other node, for
  `held_back`'s reason carried one step further: a blocked *process* exists on exactly one machine.
- **`pending?` is two arms where there are three states, and the third is where the command does
  the most.** `edit_spec` answered every non-pending run with *"it is already under way"* — so
  `offload priority` and `offload deadline`, typed at a run `offload ps` calls **waiting for a
  slot**, both said the run had begun. Two commands, one run, contradicting each other. The
  understatement was worse for the deadline: a time put on the older of two queued runs sent it
  from **first to last**, measured, while the note said the change affected "only how long the
  fleet waits for it if something goes wrong". The position now comes from
  `held_but_not_started` — the function that *decides* the order — read back after the edit, and
  the note predicts the reordering that then happens.
- **…and the surprise in that ordering is the direction.** An unstated deadline means *as soon as
  you can*, which sorts ahead of every run with a time on it — so giving a queued run a deadline
  usually sends it **backwards**. Priority has the opposite trap: slack leads and priority only
  breaks its ties, and two runs submitted seconds apart with no deadline never tie, so
  `offload priority` on them moves nothing at all. Measured: a run set to priority 10 started
  *after* one set to −5, correctly. An operator who is not told that has typed a command that did
  nothing and been congratulated for it.
- **A field an operator can set has to be readable back somewhere.** `priority` was shown in no
  report at all — `offload priority` echoed the number once and no command would say it again,
  including `offload explain`, which already prints `due` (the slack that leads) and `held back`
  (the state the pair is ordering). It is on `explain` now, under `due`, and omitted at its
  default so it is noise on nothing.
- **`offload unschedule` on a schedule already removed said `removed <id>`, again.** Exit 0, the
  whole paragraph about what had just travelled, and nothing written or republished:
  `remove_schedule` returned one `bool` for "there is such a schedule" and so answered `true` to
  both a removal and a no-op (`Removal::{Done, Already, Missing}` now). A removal command may be
  idempotent; it may not claim an act it did not perform. `offload unwatch` got this right by
  accident — a rule row is `DELETE`d, so the second try refuses.
- **…and the `home` line of a removed schedule still said `fired from here right now`**, directly
  above the `removed` line saying it fires nothing. The steward is computed for a tombstone too
  and still answers `Here`. One screen, both answers — the same defect session sixty-nine fixed
  one field over on this very report, arriving the second time through `removed` rather than
  through the steward. `steward_note` is extracted so the rule is checked rather than hoped.
- **An absent fact must not render as a gap.** `offload logs` built its agent line as
  `claude-code {version}, model {model}` straight from the agent's own init line — somebody else's
  format, read leniently — so an agent that reported neither printed `agent claude-code , model`:
  two empty slots where the facts should be. That reads as a **broken report** rather than as a
  missing fact, and sends somebody to look at the renderer instead of at the agent. It is the exact
  sentence `LogKind::TaskStarted` was split into its own variant to avoid, reached from the agent
  side. Named now (`version not reported` / `model not reported`) rather than omitted, because
  *which model did this run spend on* is a question asked of that line. The wording moved into
  `agent_line`, a pure function with a test, which is the only reason either arm is checkable.
- **A new tier of work needs a line on the screen its owner reads, and this one had none at all.**
  `offload status` prints a `resource` line per `[[resources]]` entry, *including* `NOT usable, its
  program was not found` — and printed nothing whatever for `[[tasks]]`. So of the four nominated
  kinds, three had a report naming the fault one command away (`offload sinks`, `offload triggers`,
  `offload status`) and the fourth — the only one that is a **run**, and therefore the only one a
  bid round places work on — was visible nowhere but a single `WARN` at startup on a machine nobody
  is logged into. Measured with all four broken on one daemon, which is the staging worth
  repeating: the missing one is found by reading the *other three* and asking which is absent.
- **…and a refusal that names a command has to be pointed at a command that answers.** Both new
  task refusals end *"`offload status` says which"*, and that line was true only because the same
  change added it. This is session seventy-nine's finding met while writing the fix rather than
  after it: the `Leg::Here` arm there named `offload audit` for the one case that log is empty in.
  **Type the command your sentence names, in the state your sentence is about.**
- **An attendance line is about a *person watching*, and it claimed a run was running.**
  `attendance_now`'s held-elsewhere arm said *"only bravo can tell; it is running there"* for
  every state with a holder. Two of them are not running: an `assigned` run is one the state line
  directly above calls *accepted but not started*, and an `orphaned` one is held by a node that is
  out of contact — where `Run::holder` answers `Some(last)`, *who was holding it*, which its own
  doc warns is the wrong basis for a claim about now. Two lines of one screen contradicting each
  other, which is the cheapest defect there is to see and needs somebody to look at the whole
  screen. It asks the state now.
- **`it cancelled` is the run doing the cancelling.** `RunState::name()` is an adjective, and two
  of the three terminal states read as verbs without a copula — so `nobody was asked to take it:
  it cancelled` put the run in the subject position of something it did not do. `it is cancelled`.
  One word, and the kind of thing only reading the line aloud finds.
- **Terminal is three outcomes, and the one that reopens was getting the other two's sentence.**
  `supervise` answers `Bystanding::Terminal` for all three and is right — the drop-off loop has
  nothing left to do with any of them — but `offload explain` rendered it *"it is over; nothing is
  supervising it"*, and `Failed` is the one state ADR-0013 picks back up. Measured on one daemon,
  twice, once per tier: that line printed directly above `recovery picking it up again in 29.0s
  (try 1)`. Two adjacent lines of one screen contradicting each other, in the command whose whole
  job is answering *why did that not happen*. The verdict now says only what that loop decided and
  then names **a machine** rather than a line — `whether a failed run comes back is decided on the
  node it failed on` — because `recovery` is printed on that node alone, and a sentence pointing at
  a line would be pointing at nothing everywhere else. Third time this file has recorded the same
  rule; the tell is a single sentence attached to a *predicate over several states*.
- **A pairing between a run and a node has to be right about the run first.** `offload ps`'s resume
  footnote (*"a run above says it is resumable, which is true of the run"*) and `offload explain`'s
  `resume refused here right now` both gated on `supervisor::resumable_state`, which is the
  **door's** predicate and deliberately tier-blind: `restart_task` admits a run by the same states,
  because what a run must *be* for either door to reopen it is a property of the run. A failed
  **task** is never resumed by hand (ADR-0058) — two verbs, two doors — so both reports offered the
  whole node-level pairing about a run `offload resume` refuses on every node for ever, and named a
  fleet grant or a daemon restart as the way past it. Measured on a fleet of one whose only failed
  run was a task: `Not of this node right now: this node is draining; restart offloadd to take work
  again`, against `offload resume` on a node that *was* accepting answering *"it has no checkpoint
  — there is no conversation to continue"*. A node-level, temporary-sounding refusal standing in
  front of a run-level, permanent one. `resumable_by_hand` is the reports' question — the door's
  states **and** the tier — and the clause *"which is true of the run"* is what made it a lie rather
  than a caveat.
- **A report's silence borrowed its excuse from the neighbour whose silence had a different
  cause.** `offload explain`'s `waiting` line — the ADR-0017 block, the state a run is `running` and
  making no progress *by design* — was computed on the holder and left empty everywhere else, "for
  `held_back`'s reason carried one step further: a blocked *process* exists on exactly one machine".
  `held_back` is **arithmetic** (this machine's occupancy, its own account's rate limit) and no peer
  could compute it; a pending question is a **fact its holder can be asked for**, and `offload asks`
  has canvassed the fleet for exactly that since ADR-0017 — whose own text gives *"`offload
  explain`'s reason"* as the precedent. Measured on two daemons: the peer's `offload explain` showed
  no sign of the block at all while `offload asks` on the same socket printed the tool, the clock
  and the `offload approve` line — which **travels**, so the person reading the silent screen was
  one command away from unblocking it. One canvass, bounded by the same window as the opinions
  round and skipped for a terminal run. **When a report declines to answer, check that the reason is
  the one that applies to it and not the one that applied next door.**
- **…and the attendance arm over-claimed for the third time, now about a state that is not in
  `run.state`.** With the block visible off the holder, `only alpha can tell; it is running there`
  sat one line above `stopped for an answer`. The two states fixed before this — `Assigned` and
  `Orphaned` — were both readable from the record; a run stopped mid-tool-call is `Running`, so this
  one arrives only once the fact is in hand. The `state` line still says `running`, which is the
  honest word for the process; this line is about *who is watching*, and its justification clause
  has no business restating a claim the line below it qualifies.
- **A dial to a dead peer does not fail, it times out** — and the person waits the whole window.
  `offload continue` against a parent whose node was killed took **30 s** to say *did not answer*,
  with `offload nodes` already printing `dead`. Where the view already knows a node is `Dead` or
  `Departed`, refuse without dialling; `Suspect` and `Draining` are still asked, because both are
  usually up (`fetch_blob`'s reason). Measured after the fix: 8 ms.
- **A canvass that folds every transport error into `no answer` blames the peer for this node's
  reach.** On three daemons, a bystander printed `charlie  no answer` about the healthy node
  *holding the run*, while the arbiter one screen over had its bid: the bystander had never had an
  address to dial. `ask_one` had the transport's sentence (`no known address — not discovered
  yet`) and logged it at `debug`. `Verdict::Unreachable(reason)` now, for a connection never made;
  `Silent` stays for a question that went out and got nothing back.
- **…and `explain`'s `due` line said `waiting 46.0s` about a run four lines under `state
  running`.** For a run with no deadline the age *is* the urgency (`due_at` is `created_at`), so
  the number is right and the word was not: `as soon as a node can take it` about a run a node has
  taken. A held run reads `no deadline given — submitted 46.0s ago`.
- **`nobody arbitrates it here` is not a first-second-of-gossip state.** The comment on the
  `ArbitratedBy(None)` arm said it was; restart any bystander after the home dies and it says this
  for as long as it meets nobody. It healed, measured, the moment that node met a peer that
  remembered the home as dead. The sentence now says how it ends, as `Steward::Nobody` does.
- **…and `offload checkpoint` and `offload approve` still dialled, then said the holder *is
  running* the run.** Measured on a `kill -9`'d holder: 2.0 s each, then `charlie, which is running
  this run, did not answer`, beside `ps` saying `orphaned` and `nodes` saying `dead`; `offload
  cancel` on the same run refused in 0 ms. One check now, `server::holder_out_of_reach` — the
  record's `orphaned`, or the view's `dead`/`departed` — read by all three doors. A rule enforced
  at one door of three is the three-copies shape again.
