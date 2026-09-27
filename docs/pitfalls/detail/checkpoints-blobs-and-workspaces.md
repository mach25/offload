# Checkpoints, blobs and workspaces — full entries

The working rules are in `../checkpoints-blobs-and-workspaces.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **Mid-turn is not a safe checkpoint.** An agent halfway through a tool call has state in
  the tool, not the transcript. If a drain deadline hits mid-turn the honest options are wait
  or lose the turn — never snapshot anyway.

- **A field written twice in a run's life is a field that is wrong in between.**
  `RunProgress::workspace` is documented as "the holder's summary of the run's worktree — 2
  modified, 1 new", and it was set to `preparing` at launch and to the truth once the leg had
  *ended*. So `offload ps` said `preparing` for the whole of an overnight run — in the column
  beside the one saying whether that work is replicated, which is the pair somebody checks at
  07:00 — and it gossips, so the whole fleet reported a worktree as half-built while an agent
  worked in it. Refreshed at **every** turn boundary, not at every checkpoint: tying it to the
  capture would be two turns stale under `every_turns = 3` and permanently wrong under
  `every_turns = 0`, which is the bug unchanged for anyone who turned automatic captures off.

- **…and the same arithmetic on the checkpoint side.** `Selection::summary` counted build
  output and called *everything else* "skipped as too large", so a file left behind because the
  checkpoint's total budget was full was reported as one that was too big — two settings, and
  the message named the wrong one. Three reasons, three counts. The line is the whole of what
  the operator is told about work that did not travel.

- **A status decides where work *goes*, never who is worth *asking*.** `fetch_blob` swept only
  peers it marked `Alive` — while the doc comment directly above it argues that availability must
  not be gossiped *because* an advertisement is stale by the time it is used, and a node that
  lacks the bytes says so in a millisecond. A node's liveness is exactly as stale. And the moment
  a checkpoint is needed is the moment somebody has gone quiet, so the filter skipped precisely
  the peers most likely to hold it: `Suspect` is not unreachable — the state exists to be argued
  with (ADR-0007) — so the fleet's only copy could sit on a node that answered every dial and was
  never asked, and the migration failed with "no peer could supply the blob". Ordered now, not
  filtered: the hint first, then live peers because they are likelier, then everyone else. The
  code had already conceded the point for `from`, which is asked whatever its status.

- **A checkpoint stops being one the moment the run stopped by decision — and the collector kept
  exactly the expensive one.** ADR-0022's own safety argument is that recovery fetches the blobs
  the *record* names, so anything the record has moved past is unreachable; the half nobody drew
  is that this applies to the checkpoint the record **still** names, once nothing can start an
  agent from it. `Supervisor::resume` refuses `Completed` and `Cancelled` by name, and nothing
  else fetches a checkpoint at all — so the tick reclaimed the five cheap superseded transcripts
  and kept the biggest one, for ever, on the home node, on the leg that ran it, and on every peer
  that took a replica. Measured: **472 KB a run, and session twenty-five wrote the same number
  down as success** ("the store settling at exactly one blob per completed run"). Zero is the
  right number. `Run::resumable_checkpoint` is the predicate, over `RunState::may_resume_later`
  whose arms are written out — a new terminal state must *decide*, because answering "no" for
  something resumable deletes the only copy of a conversation and the compiler is the only thing
  that will ask. `Failed` is the exception and the reason it is not `is_terminal`: it is the one
  terminal state that reopens, so its checkpoint is the one whose loss cannot be undone.

- **…and `is_terminal` was the same wrong question in two reports, about the same run.** A failed
  run is terminal and it reopens, so it is precisely the run whose checkpoint decides whether the
  work survives its machine — and `offload ps` suppressed the `here only` warning for it while
  `offload explain` printed a bare turn number. The warning's own text ("if it goes away, so does
  the work") is written for that row and no other. Extending both needed
  `Checkpoint::is_durable(Option<NodeId>)`, because a run with **no holder** has nothing to
  discount and `replicas` never contains the node that took the checkpoint: the question is just
  whether anybody else has it. Both callers had invented a holder instead — one substituted the
  local node, which answers about a peer's copy on a peer, and one read no holder as no copy,
  reporting three replicas as `on one machine only`. The general shape: when a predicate is
  *nearly* the one you want, the case it gets wrong is the one nobody wrote a test for.

- **"Deliberately manual" is the same premise a third time, and rows were the smaller half.**
  `blobs::collect_garbage` was careful, property-tested, and called by nothing, because it was
  "deliberately conservative and deliberately manual" — and manual means somebody runs it, on a
  machine nobody logs into. Measured: one six-turn run left **six blobs totalling 851 KB, of which
  the surviving checkpoint referenced one, at 243 KB.** `Run::record_checkpoint` *replaces* the
  checkpoint and a transcript is the whole conversation so far, so the waste is quadratic in the
  conversation rather than linear, and it is every run on every node rather than anything to do
  with triggers. It runs on a tick now (ADR-0022), every fifteen minutes with an hour of grace —
  the grace because a blob is written *before* the row that references it, in `capture` and again
  on the receiving side of a replication. What makes deleting a superseded replica safe is that
  **recovery fetches the blobs the *record* names**: the moment the record moves past a
  checkpoint, nothing knows how to ask for it again.

- **Uncommitted work is the valuable part.** A migration that moves a git ref but drops the
  dirty worktree has lost exactly what the agent was doing. Untracked files count: a newly
  written source file is untracked, and `git status` without `--untracked-files=all` won't
  show it.

- **A worktree that is still here is not necessarily the current one.** `adopt` inferred
  "current" from "exists" — right for the run that never left, and false for the one this
  project is about, because nothing removes a worktree when a run leaves (`remove` is called
  only from `cleanup`, manual and terminal-only). A run that checkpointed here at turn 1, did
  nineteen turns elsewhere and came back was adopted at turn 1, with its bundle and patch never
  applied and its transcript skipped for the same reason at the same path — silently, the log
  saying "adopted in place". Turn counts are what order two legs of state with no clock and no
  message, so the worktree records the turn it holds and `adopt` answers `Current | Superseded |
  Absent`. **Unknown is not current** (same rule as the collector, and as auto-resume), and a
  superseded checkout is *moved aside* rather than deleted, because it may hold the only copy of
  a mid-turn edit.

- **A list of directory names is a guess about somebody else's repository, so it loses to what
  that repository says.** `target/` is build output; `build/` holds four hand-written files in
  an ingress chart, WordPress tracks 132 files under `wp-includes/js/dist/`, and plenty of PHP
  and JS repos commit `vendor/` and `node_modules/` outright — measured on the repos of one
  laptop, which is what open question #3 was waiting for. So a name only means build output
  while git tracks nothing in *that* directory, asked per directory so `build/target/debug/x.o`
  is still caught. The residual is stated rather than discovered: a directory the agent creates
  from scratch whose name is on the list is still excluded, because there is nothing to ask.

- **Unknown is not none, in a collector.** A run row this build cannot decode has *unknown*
  references, and skipping it collects everything it points at. `collect_garbage` fails the whole
  pass instead. Same shape as `AttendanceUnknown` refusing to auto-resume: where the safe answer
  and the convenient answer differ, a collector has to take the safe one loudly.

- **Never `clone --mirror` a repo cache.** Its `+refs/*:refs/*` refspec means the next
  `fetch --prune` deletes every `offload/run-*` branch, because none of them exist
  upstream. Upstream refs belong under `refs/remotes/origin/*`, leaving `refs/heads/*` ours.

- **A local-path repo pins a run to one machine.** `RepoSource::portability()` says which
  kind you have; it's an eligibility fact, and discovering it at resume time means the
  migration already failed.
- **A checkpoint's transcript can be behind the run it is checkpointing.** Measured while building
  ADR-0040: the capture at the turn-1 boundary wrote a 16,999-byte transcript blob containing
  `queue-operation`, `user`, `attachment`, `atis-latch` and `ai-title` rows and no assistant rows
  at all, because the agent flushes the file behind its own event stream by an amount it does not
  promise. Nothing failed and nothing was empty — the blob is a perfectly good capture of a
  transcript that had not caught up, which is precisely why this had gone unnoticed. The
  consequence was then walked, and it is worse than the guess in this entry's first draft — which
  said the agent would probably redo a turn. It does not. Resumed onto a rebuilt worktree (the
  migration path: `transcript restored from checkpoint` in the log, the stored blob written over
  the agent's own current file), the agent replied **"No response requested."**, made no tool
  call, wrote nothing, and the run reported **`Completed`** — an abandoned run that reads as a
  success, reached from a new direction. The control, whose capture had caught up, resumed and
  wrote the nanosecond timestamp that had existed only in the conversation, so the mechanism is
  right and it was the bytes that were not there.

  The lag is a **race, not a fixed offset**: two runs captured at the same turn 1 stored 22,953
  bytes with the conversation and 17,087 bytes without it. So `checkpoint` now refuses a capture
  whose transcript has no assistant messages in it, which is the rule the file already stated —
  better to fail loudly than record a checkpoint that cannot do its job — applied to the case the
  existing check missed, because it asked whether the *file* was there rather than whether there
  was a conversation in it. Walked after the fix: turn 1 refused and said so in the run's own log,
  turn 2 captured and released, resumed onto a rebuilt worktree, and the nanosecond value
  survived. **Being about a turn behind is the normal state** — measured at every boundary of a
  healthy five-turn run — so only *nothing* is fatal, and a warning on the lag was removed for
  firing on every run.

- **The capture at a turn limit had no retry behind it, and the rule above assumes one.** Every
  entry above this one about a refused capture ends "the next boundary tries again", and that is
  what makes refusing an empty transcript harmless. ADR-0039 added a second caller for which it is
  false: the turn limit captures at the boundary it *stops* on, unconditionally and on purpose
  (§4 — "a run somebody capped is precisely the run they want to look at"), and there is no later
  boundary. So the race the guard was written to lose safely became permanent exactly where the
  work mattered most.

  Found by walking the two things session thirty-one built *against each other*, which neither
  walk had done: the guard was walked, and the limit was walked, and the seam between them was
  not. The clearest single piece of evidence was one `offload ps` row — **`TOKENS 30`, `SAFE -`** —
  the same transcript read twice, at two different moments, disagreeing about whether the
  conversation was in it. `note_tokens` reads it after the process is down and got the numbers;
  the capture read it before and got nothing. Session thirty-one built the late read for the
  reporting column and left the checkpoint on the early one, and `note_tokens`' own doc comment
  states the fact that makes this certain — *"the run it is emptiest for is the short one, which
  is exactly the run somebody caps."*

  Measured on claude 2.1.251 / haiku 4.5, `--max-turns 1`, real agent: **2 of 3 runs lost the
  checkpoint** on the unfixed build. The failure is **binary rather than gradual** — every
  transcript that had not caught up was *exactly* 24,188 bytes with zero assistant rows (three of
  them, byte-identical: the session preamble), and every one that had was 29.8–31.4KB with two or
  three. There is no partial state to read: either the agent has flushed the message it just spoke
  or it has not.

  The first fix was wrong in an instructive way. It stopped the agent and *then* captured, on the
  premise `note_tokens` is built on — once the process is gone the file is final. That premise is
  true and it is not sufficient: a `SIGTERM` arriving before the agent has flushed the turn it
  just spoke loses that message **outright** rather than writing it on the way out. The proof is
  that those 24,188-byte files still hold zero assistant rows now, long after the processes that
  owned them exited — final, and empty. It measured 3/4, which is the kind of improvement that
  looks like a fix and is not one. So the wait has to come **before** the stop: after it, nothing
  more is ever coming. `await_transcript` polls the file every 100ms for up to 3 seconds, then
  stops the agent, then captures. Measured after: **8 of 8 kept**, the 3-second timeout never
  fired, and boundary-to-checkpoint stayed between 0.66s and 1.27s — the flush is sub-second when
  it happens at all, and the old code was simply reading before it.

  Stopping before capturing is kept for its own separate reason: the patch is then taken from a
  worktree nobody is still writing into. The guard is
  `the_capture_at_the_turn_limit_waits_for_the_agent_to_write_the_turn`, whose fake agent gains
  its assistant row one second after the boundary; it was watched red with the wait removed.

- **The checkout sweep read its guards, awaited a subprocess, and then acted on what it had
  read.** `Supervisor::reclaim_departed_checkouts` is the one place in this tree whose effect is
  `git worktree remove --force`, and ADR-0023's doc comment enumerates five guards in front of it —
  including, in as many words, *"no agent of ours is live on it … that process is writing into this
  very directory."* All five were taken at the top of the loop body. Between them and the removal
  sat `holds_uncommitted`, which is `git status --porcelain=v1 --untracked-files=all -z` in a
  `tokio::process::Command`: **3.0ms median on a small warm repo, 24ms with 20k untracked files**,
  measured before the test was written. That is not a scheduling hazard at the margin, it is an
  unconditional yield point with a guaranteed multi-millisecond floor, and every fact the guards
  read can change inside it.

  What changes it is `Supervisor::resume`, whose two callers session thirty-three already named — a
  person typing `offload resume`, and the recovery tick picking an unattended failure back up. Its
  durable half is `Store::update_run` (assign: holder becomes this node) immediately followed by
  `register` (the `live` entry, `cancel: Some`), two **synchronous** calls with no await between
  them. So the transition from "reclaimable" to "an agent is being launched into this directory" is
  atomic with respect to the sweep, and it lands whole inside the sweep's window.

  Measured: 40 attempts, four worker threads, the racer doing exactly `reopen` + `assign` +
  `register` one millisecond into the `git status` — **40 of 40 checkouts reclaimed**. Worth
  contrasting with session thirty-three's tripwire on `resume` itself, which never tripped in 200
  aligned attempts and was fixed structurally anyway: this one is the opposite, a window wide
  enough that the loser is the default outcome. A 3ms subprocess against two store calls is not a
  race anybody wins by luck.

  The blast radius is wider than the failed run the test races. The first door is `moved_on` —
  *another leg produced the run's latest position* — and a run parked **`Pending`** here after a
  migration satisfies it, because a `Pending` run holds no lease and its last position came from
  the other leg. That is the ordinary state of every run this node has ever handed on, so the racer
  is not only the recovery tick on its 5s–5min backoff against a 15-minute sweep; it is a person
  typing `offload resume`, which has no schedule at all.

  The fix is the shape `refuse_unresumable` already has either side of `Store::update_run`'s lock:
  one function, `Supervisor::reclaimable`, asked **twice**. The first call is only there to decide
  whether the subprocess is worth spawning — the sweep walks every checkout on the disk and most
  belong to runs it will keep — and the second, on the far side of the await with nothing between
  it and `remove`, is the one that decides. It returns the `Run` rather than a `bool` so the caller
  does not go back to the store for a third, differently-timed copy of the same facts.

  The guard test took three goes and the two failures are the useful part. The racing form that
  found this — a 1ms pickup against the 3ms `git status` of a two-file fixture, 40 attempts — went
  red **2 times in 8 full-suite runs** after the fix, because under real parallelism the sleep can
  outlast the subprocess and the sweep removes the checkout *correctly*. Replacing it with a purely
  deterministic version was worse: asserting `reclaimable` before and after the pickup **passes with
  the fix removed**, since the state has to change *inside* the loop and a test that changes it
  beforehand only exercises the first call. What works is widening the window rather than abandoning
  the race — `git status` must re-hash a file whose mtime moved and still reports the worktree
  clean, so rewriting a 16 MB file with identical bytes buys **83ms** against the 1ms pickup instead
  of 3ms. Watched red with the second `reclaimable` removed, and green across nine full-suite runs,
  three of them under full CPU load.

  Two things this did *not* turn out to be, both checked rather than assumed. `reclaim_occurrence`,
  the trigger path to the same removal, is safe — it goes through `cleanup`, which reloads the run
  and refuses a non-terminal one, and that reload *is* a re-check. And a terminal run can never
  have a live agent here, because `release` clears the `cancel` handle before the terminal
  transition is written, which is what makes `cleanup`'s single `is_terminal` guard sufficient
  where the sweep needed four. The guard test carries a control — the same fixture, nobody racing
  it, asserted reclaimed — because a `reclaimable` that answered `None` for any unrelated reason
  would otherwise pass forty attempts by never removing anything at all.

  Found by running the search `docs/HANDOFF.md` carried, pointed at the candidate it named that was
  still unwalked: `offload rm` against the collector. The shared state is the checkout, not the
  blob, and the two callers are the sweep and the resume.

- **…and `cleanup`'s own guard is already next to its effect.** Written down because two handoffs
  in a row suspected otherwise, on the reasonable-looking argument that `cleanup` takes its
  `is_terminal` check and then awaits `git worktree remove --force` while this sweep takes its
  guard **twice**. The difference is not that one is careful and the other is not: this sweep has
  `holds_uncommitted` — a `git status` subprocess — *between* its guard and its effect, and
  `cleanup` has nothing between them at all. `load_run`, `is_terminal` and `RepoSource::parse` are
  synchronous, and `remove` is the next statement; `offload rm`'s own handler resolves the id and
  calls straight in. A second check there would be adjacent to the first. **It is the await that
  makes the gap, not the effect being dangerous** — which is also why the fix here was a second
  `reclaimable` and not a lock.

  The residual both share is the one no check closes: `remove` itself spans an await, so a resume
  starting *inside* the removal would need mutual exclusion rather than a guard. **That is now
  built** — see the next entry, which measured it and found the window is lost every time, not
  narrowly.

- **A guard cannot be put inside somebody else's program, so the last window needed a lock.** Both
  teardown doors were guarded correctly — `cleanup`'s check is the statement before its effect, the
  sweep asks `reclaimable` twice — and both were still open, because the effect is
  `git worktree remove --force`: a subprocess, a few milliseconds long, deleting a directory tree.
  A read before it is on the wrong side of it, and there is nowhere later to put one.

  Measured on a forced pair in a scratch repo, no daemon: `prepare` a worktree, write an
  uncommitted file into it, start `remove`, then start a rebuild — `adopt`, falling through to
  `prepare` — a fixed number of milliseconds later, sweeping the delay. **10 of 60 passes lost, and
  every pass inside the window lost**: 10 of 10 at 0–1ms, 0 of 50 at 2ms or later. ~2ms is how long
  the removal takes on this machine. What the loser gets is `Adoption::Current` for a directory
  that is gone by the time `adopt` returns, "adopted in place" in the run's log, an
  `install_transcript` that skips the install on the strength of that word, and an agent spawned
  into a `cwd` that does not exist.

  ADR-0052 is the answer and its shape is the part worth carrying: `WorkspaceManager::hold(run)`
  returns a `CheckoutGuard`, and `prepare`, `adopt`, `supersede` and `remove` are methods **on the
  guard**, taking the run id from it. So the compiler asks for the lock — a rule enforced by a doc
  comment is a hope, and the id coming from the guard means a guard for one run cannot change
  another run's checkout. It is held across the *sequence* (`restore_workspace` asks three of the
  four in a row; a lock inside each would protect none of the gaps between them) and dropped
  before the agent starts, because `register` claims the run in `live` before `launch` spawns
  `drive`, so from that point every teardown door's own guard already refuses. That is the closing
  argument, and it is worth being able to state: a rebuild that has started **holds** the checkout,
  and one that has not started has already **registered**. No third moment.

  Three details that cost something to find. **`try_hold` for the sweep, `hold` for the rest**: the
  `hold` variant *deadlocks the sweep* behind whoever holds the checkout, which for a resume is a
  bundle fetch — a background tick blockable for the length of an unrelated operation is worse than
  the bug being fixed, and a checkout somebody is rebuilding is one the sweep must leave alone
  anyway. **`cleanup` takes the hold before the load**, because taking a lock is an await and the
  other order would put its terminal check back on the far side of one — undoing, while fixing a
  race, the very property the entry above this one establishes. **The map of mutexes is behind an
  `Arc`**: `WorkspaceManager` is `Clone`, and a lock a clone does not share is not a lock.

  Both doors have a test that was watched red: the workspace crate's walks the window, the node's
  is deterministic, and one line — making `mutex_for` hand out a fresh mutex each time — fails
  both. That one-line revert is also the cheapest A/B for anything else built on this lock.

- **"How often is a checkpoint not durable when a node leaves" is a state question, not a timing
  one.** ADR-0043's third residual asked for this measurement before anything was built, on the
  reasoning that background replication starts at the turn the checkpoint was taken, so the answer
  might be "only when there was never a peer". Measured on two daemons over loopback, three runs,
  `every_turns = 1`:

  | newest blob | recorded → replicated |
  | --- | --- |
  | ~1 KB | 1.3–6.7 ms |
  | ~1 MB | 6.4 ms |
  | 11.5 MB | 47.3 ms |

  **30 of 30 checkpoints replicated**, every one inside a turn gap of seconds, and the peer's blob
  store ended byte-identical to the holder's (32 blobs, 97,613,840 bytes on both). Scaling is
  linear and unremarkable — about 244 MB/s on loopback, which is the number a real link will not
  match, and the one to re-measure before trusting this on wifi.

  So the timing answer is "never, with a peer". **The state answer is the one that matters**, and
  it is worse than the residual assumed. `Supervisor::replicate` has exactly one production
  caller — `checkpoint` — and there is no retry and no background pass. A checkpoint taken while
  no peer was reachable stays `here only` **for ever**, not until a peer turns up: measured with
  four checkpoints taken alone, then a peer started and both nodes confirming each other `alive`,
  and all four still `here only` — only the *next* checkpoint replicated. So the residual's
  "only when there was never a peer" should read **"whenever no peer was reachable at the moment
  of the last checkpoint"**, and for a run that has *failed* there is no next checkpoint to fix it.

  The failure it leads to is reachable end to end and was walked: alpha alone, run fails, `offload
  drain` → `left for a person: this node is leaving and its conversation exists nowhere else, so
  nobody could continue it`, which is the honest answer and the right one. Whether forcing
  replication on the way out is worth building is still open — but the thing to weigh is no longer
  "how fast are the bytes", it is that a departing node is the last chance that copy will ever get.

- **A rule with two doors and one guard is a rule about one door: `resume` never fetched.**

  Every restart of an agent from a checkpoint goes through one of two functions.
  `Supervisor::start_run` — a submission, or a grant from an arbiter — calls `prepare_start`,
  which calls `ensure_blobs` before building `Start::Resume`. `Supervisor::resume` — which is
  both `offload resume` and the recovery tick's auto-resume — built the same `Start::Resume`
  itself and went straight to `launch`. The handoff that carried this as a lead had ruled the
  obvious explanation out with the sentence *"`prepare_start` does call `ensure_blobs` on the
  resume path (it is the only place `Start::Resume` is built)"*, which was true of one door and
  cost the lead two sessions. One `grep -n "Start::Resume"` says otherwise, and it is the same
  shape of miss as ADR-0051: the guard was correct, and it was on one of the ways in.

  **Walked, twice, against the pre-fix binary on identical state.** Alpha alone hosts a run to
  turn 5, `offload checkpoint` releases it `pending` with the checkpoint `here only`; bravo
  starts, both nodes `alive`, bravo holds the run row and **zero** blob files. `offload resume`
  on bravo:

  | | pre-fix | with `Restorable` |
  | --- | --- | --- |
  | the CLI says | `resumed 01a0708be323` | `Error: agent: fetching checkpoint blob 28ca6440: … no peer could supply the blob` (peer down) |
  | the log says | `workspace ready`, then `run failed … state store: blob bf5035b4… is not stored here` — **no fetch line anywhere** | `fetching checkpoint blob run_id=… blob=bf5035b4`, then turn 5 → 12 |
  | the run ends up | **`failed`**, reopened and at a spent epoch | `pending`, byte for byte as it was (peer down) / `running`, `replicated` (peer up) |

  Two details worth keeping. The failure is 12ms after an `Ok` to the operator, so the command
  and the failure are two screens apart — which is why an earlier walk recorded "two occurrences
  got different distances, one past `workspace ready`, one before it": those are the two
  `get_blob` sites, `install_transcript` just after that line and `restore_workspace`'s bundle
  just before it. And the pre-fix path is not merely a failure to start: it *reopens* a `Failed`
  run into `Assigned`, burns an epoch, builds a worktree and then fails the run again, so an
  operator retrying gets a fresh worthless leg every time.

  The fix is a type, not a call. `restorable::Restorable` wraps the `Checkpoint` that
  `Start::Resume` carries, its only constructor is `Restorable::check(cp, |h| here(h))` — which
  cannot be satisfied without an answer for every blob — and `ensure_blobs` is the only caller.
  A comment saying "call `ensure_blobs` first" is a hope; this is the door. The placement in
  `resume` is **ahead of the claim**, unlike `start_run`'s: everything it can go wrong with is a
  refusal this run would meet whoever asked, so no epoch is spent, no lease is taken, nothing has
  to be given back by hand, and a `Failed` run is not reopened only to sit there.

  And `ensure_blobs` now asks the store **twice**, the second answer deciding. A `fetch` that
  returns `Ok` without leaving the bytes here is indistinguishable from one that worked; the place
  that would find out is `restore_workspace`, three awaits later and with a worktree half built.
  The second ask costs one `is_file` per blob.

  **What this says about the walks, which is the larger finding.** Migration to a node that has
  not already been replicated to is the thing on the front page of this repository, and it had
  never been walked: every migration walk so far started both daemons early enough that the
  checkpoints replicated, so `has_blob` was always true and `ensure_blobs` always a no-op. The
  staging that finds it is one line different — **start bravo after the checkpoint, not before**.

- **"The branch is here" is not "the work is here": the bundle a migration silently dropped.**

  Full reasoning, alternatives and residuals in **ADR-0053**; this is what to carry.

  `restore_workspace` decided whether to apply a checkpoint's bundle from
  `WorkspaceManager::has_branch` — apply it only if the run branch is *absent* from this node's
  mirror. The comment said the commits "are already here", and it was guarding something real:
  `checkpoint::restore` ends in `git reset --hard`, so applying a bundle over a branch that has
  moved *past* the capture walks the run backwards. A checkpoint is taken at a turn boundary and
  the agent keeps working, so a branch ahead of the newest capture is the normal state of a live
  run.

  But `has_branch` answers "has this node ever run a leg of this run", and nothing keeps that
  branch current. So a branch that was merely **older** than the checkpoint suppressed the bundle
  exactly as thoroughly as one that was newer.

  **Walked on two daemons, three legs, no crash and no error anywhere.** Leg one on alpha: four
  turns, four commits, `offload checkpoint`. Leg two on bravo: `rebuilt from bundle and patch`,
  alpha's four files arrive, four more committed. Leg three back on alpha: **`re-checked out,
  patch reapplied`**, and the checkout holds four of the eight files. The bundle blob was on
  alpha, was named by the checkpoint, and was never opened. An earlier six-turn pass had
  `git rev-list` on the two branches intersecting in **exactly one hash** — the base commit.

  Two ways in, and the migration is the common one. The other: `prepare` creates the branch at the
  base commit before `restore` runs, so any leg that dies in that window leaves the branch at base
  and **every later resume on that node skips the bundle for ever**. It is a state, not a race.

  The fix moves the decision into `restore`, which fetches the bundle into `refs/offload/restored`
  and then asks `git merge-base --is-ancestor refs/offload/restored HEAD`. `BundleOutcome` carries
  the answer back as `Absent | Applied { left_behind } | AlreadyHere`, because the run's log has to
  tell *left alone on purpose* from *there was nothing to apply* — and when the reset moves past
  commits this checkout had and the bundle does not, the old tip is named, since it survives only
  in the reflog. `has_branch` keeps its other job: choosing the start ref, where an existing branch
  is still right because it may hold a leg that is ahead.

  **How it was found, which is the transferable part.** Not by reading the code. It fell out of an
  A/B for a *different* fix: the control binary failed a resume midway, which left the branch at
  base, and the fixed binary's next resume then reported `re-checked out, patch reapplied` on a
  checkpoint that plainly had a bundle. The tell was one word in a log line that nobody had a
  reason to distrust. Two things follow: **read the account a resume gives even when it succeeds**,
  and **when a fixture sets a field to `None`, ask what `Some` would have done** — the run-comes-back
  test, which stages precisely this, sets `bundle: None` and puts the far leg's work in an
  uncommitted file.

- **A same-node resume normally never reaches `restore`, which is why the `AlreadyHere` arm looked
  unwalkable.**

  ADR-0053 shipped with its `AlreadyHere` arm covered by a unit test and not by a walk, and the
  handoff said the staging would be "a daemon killed after a checkpoint and several more turns,
  then restarted". That staging does not work, and the reason is worth keeping: `adopt` reads the
  worktree's **turn marker**, which is written at the last *capture* rather than at the last turn.
  Kill the daemon at turn 8 with captures every 5 and the marker reads 5, the checkpoint reads 5,
  `at >= through` holds, and the answer is `Adoption::Current` — **`adopted in place`**, with
  `restore` never called. The checkout on disk is genuinely current, so this is correct; it just
  means the arm is unreachable from there.

  What reaches it is the case this ADR's context already names: **the branch outliving the
  worktree**. `every_turns = 5`, kill timed between captures so the branch is three commits past
  the checkpoint, then remove the checkout. Measured against a build with `if already && false`,
  on the same state directory:

  | | guard removed | with the guard |
  | --- | --- | --- |
  | the log says | `rebuilt from bundle and patch; this branch was at cef5d49, which this checkpoint does not contain` | `the commits here already held this checkpoint; patch reapplied` |
  | files kept | `work-L1-1..5` — turns 6, 7 and 8 gone | `work-L1-1..8` |

  Two things the walk cost before it worked. **`offload ps` trails the agent**: a pass that read
  `turn 7` and killed immediately caught the run at turn 10, on a capture boundary, where the
  branch equals the checkpoint and nothing is being tested. Time the kill off a file the fake agent
  has just committed. And **`rm -rf` is not how the product removes a worktree**: git keeps the
  registration and the next `prepare` dies with `'offload/run-…' is already used by worktree at
  '…'`, which is the leftover entry two rules above, met from the walk's side. `supersede` and
  `remove` both run `git worktree prune`; a walk deleting a checkout by hand has to as well, or it
  is testing its own staging.

- **A departing node pushed nothing, so `is_durable` decided the strand from a fact nobody had
  tried to change.**

  Full reasoning, alternatives and residuals in **ADR-0054**; this is what to carry.

  `decide_recovery` asks `fleet_could_start`, which asks `Checkpoint::is_durable`, to choose
  between handing a failed run to the fleet and stranding it — *"this node is leaving and its
  conversation exists nowhere else, so nobody could continue it"*. Honest, and decided by a fact
  that `depart` had never attempted to improve. `Supervisor::replicate` has one caller
  (`checkpoint`), no retry and no background pass, so a capture taken while no peer was reachable
  stays `here only` for ever — and a failed run has no next capture, nor does a `Pending` one
  released by `offload checkpoint`.

  So *"there was no peer when the capture happened"* and *"there is a peer now"* were routinely
  both true at the moment a node left, and nothing joined them up. Walked on two daemons with the
  same staging on both arms — alpha alone, run fails at turn 4 with its only checkpoint here,
  **then** bravo starts, then `offload drain`:

  | | before | after |
  | --- | --- | --- |
  | `offload drain` says | `nothing to hand over` | `copied 1 checkpoint(s) nobody else had to a peer` · `1 failed run(s) given back to the fleet` |
  | the run ends up | `failed`, `here only`, on the machine that is leaving | **`running` on bravo at turn 5**, `replicated` |

  Three things about the shape, because each was a decision. It runs **before**
  `hand_back_failed_runs`, since that is the pass whose input it changes. It covers **every** run
  with a `resumable_checkpoint`, not only failed ones, because a `Pending` run strands one door
  over — a peer wins it later and fetches from a node that has gone. And every failure is **soft**:
  a copy that could not be pushed leaves the run exactly as stranded as it was, reported through
  the same sentence, so the pass can improve an outcome and never worsen one.

  **The bug it shipped with, caught by its own test at the first run**, is worth more than the
  feature: `list_runs(false)` means *skip terminal rows*, and a `Failed` run is terminal — so the
  first version of the pass could not see the runs it exists for. That is the `is_terminal` entry
  four rules up, in a new function, three sessions after it was written down. A rule being in the
  pitfalls file does not stop you re-deriving the mistake; having a test that stages the *actual*
  case does.

## A rescue that destroys the evidence for its own necessity keeps everything for ever

**The mechanism.** `CheckoutGuard::supersede` exists because a checkout the fleet has moved past
may hold the only copy of a mid-turn edit, so it renames rather than removes — and then deletes
the rescued copy's `.git` file, correctly, since it points at a worktree registration the
following `git worktree prune` is about to drop. Those two lines are three apart and the second
undoes the first's option value: after the rename the directory is a plain pile of files, and
`holds_uncommitted` — the question `reclaim_occurrence` uses to tell litter from treasure — cannot
be asked of it again by the product or by anybody else.

**The measurement.** `git status` inside a rescued copy answers `fatal: not a git repository`. Run
one run through three legs at the crate level, alternating a far leg that commits with a near leg
that diverges, and the state directory holds three `<run>.superseded*` directories and three
`refs/offload/left-behind/<run>/<short>` refs, plus one `offload/run-*` branch per run ever
hosted. Nothing in the product removes any of them, and `free_superseded_path` counts to 1,000
before it gives up and lets the rename fail.

**Why it stayed.** Renaming unconditionally reads as the cautious choice, and the doc comment
argued for it in exactly those terms. It is not: it does not defer the decision, it makes it, in
the direction of keeping every copy — and every kept copy then looks identical to the one that
mattered. A clean superseded checkout is redundant by *measurement* rather than by judgement,
because its commits are on the run branch in the mirror and `remove` keeps that on purpose.

**The walk.** One daemon, both arms on a byte-identical state directory, with the condition forced
false for the control (`&& false`, session fifty-nine's technique). A fake agent that commits each
turn, checkpointed at turn 5, left `pending`; the turn marker deleted, which is what makes `adopt`
answer `Superseded { at_turn: None }` on one machine — the *run in flight across an upgrade* case
the marker exists for, and the cheapest staging that reaches `supersede` at all, since on one
daemon a capture and the marker stay in step and a `kill -9` leaves them equal. Then
`offload resume`.

The control kept `<run>.superseded` for ever and said `the checkout here held turn unknown and
uncommitted work, so it is kept at …` — **false about a clean checkout**, which is what an
unconditional rescue leaves a report no way to avoid saying. Every one of the six files in it was
checked against the run branch and every one was on it. The fixed build removed it, said
`nothing uncommitted, so it was removed — its commits are on the run branch`, and the run resumed
from turn 5 to 8 and completed with all five earlier commits intact — identically to the control,
which is the point.

**And the other half of ADR-0053's residual was never the fleet's business.** Both rescues were a
`tracing::warn!` on an unattended machine plus a sentence in the *run's* log — served by the run's
**holder**, while the leg doing the rescuing is by construction the leg that lost the run, and
unreadable by anybody once the run is deleted. `RunProgress` cannot carry it for session
seventeen's reason: one fleet-agreed value, one writing leg, and the write would be correctly
dropped on the very node that did the rescuing. That is `AuditEvent::Reclaimed`'s whole argument,
so its counterpart is `AuditEvent::Rescued`, with a third `Reclamation::Redundant` door for the
removal. Neither row carries a path, because both names are derived from the run id.

**How it was found.** Reading ADR-0053's residual as a description of what is true now rather than
of what was decided — the habit session sixty-one's entry asks for — then measuring the accumulation
instead of arguing about whether it mattered. The `.git`-deletion consequence was not in the
residual, in the ADR, or in the doc comment; it took one `git status` in a rescued directory.

- **The blob plane moves whole blobs, in memory, with a 512 MiB cap — and ADR-0061 wants 6.5 GB
  through it.**

  ADR-0061 left one thing to measure before building: *"Whether acquisition is resumable. Blobs are
  all-or-nothing today, and a large archive interrupted near the end should not start over."* The
  measurement was done on two daemons with a fake agent that commits a 256 MiB file, so the bundle
  blob carries it — which is the ADR's own shape, a workspace travelling as bytes.

  Three numbers, and the first one settles the question before resumability is reached:

  **The cap is 512 MiB and the motivating workspace is 6.5 GB**, thirteen times over. It is
  enforced on both sides (`Cluster::handle`'s `BlobPush` arm and `fetch_blob`'s `BlobFound` arm),
  and the reason is good: a peer's `size` field is attacker-controlled, so without a cap
  `BlobFound { size }` is an allocation request. Raising it is not a one-line change.

  **Nothing partial survives an interruption.** The receiver was `kill -9`'d mid-transfer of the
  256 MiB bundle; afterwards its blob directory held **363 bytes**, the previous turn's transcript
  blob, and nothing else. `recv_bytes(size)` fills a single `Vec<u8>` and `blobs::put` writes to a
  temp file and renames, so a failure leaves neither a partial row nor a partial file. That is
  correct — there is nothing half-written to be mistaken for a whole blob — and it means resuming
  would need a protocol that frames a blob in pieces, a resumable transfer and a streaming store
  API. A wire version and a separate decision.

  **A blob costs its whole size in memory at both ends.** Peak RSS across the transfer, sampled
  twice a second:

  ```
  baseline   alpha 33 MB   bravo 32 MB
  peak       alpha 308 MB  bravo 298 MB      (268,517,749 byte bundle)
  ```

  which is what `async fn get(&self, hash: BlobHash) -> Option<Vec<u8>>` says it will cost. The
  trait's own doc comment got there first: *"Whole-blob rather than streaming, because a checkpoint
  is megabytes… If something ever puts a repository in here, this is the signature to change
  first."* ADR-0061 is exactly that, and the comment is the thing to read before the ADR.

  Throughput for scale: 6.36 s for 256 MiB over loopback, ~40 MiB/s, so 6.5 GB would be ~2.7
  minutes over loopback and worse over wifi — with the run neither here nor there throughout.

  The reusable part is not about blobs: **a doc comment that names its own breaking point is a
  measurement waiting to be taken, and the ADR that proposes crossing it should cite the number.**

- **A checkpoint too big to replicate said "nobody would take a copy".**

  Measured by lowering `MAX_BLOB_BYTES` to 64 MiB under the same 256 MiB bundle — the real cap with
  a real workspace, at a scale that fits on a laptop. The whole of what an operator saw:

  ```
  INFO  checkpoint recorded … turn=2 summary=268517748 byte bundle (0 untracked file(s))
  WARN  checkpoint is on this node only; nobody would take a copy run_id=…
  ```

  The peer was alive, meshed, willing and had just accepted the turn-1 blob. "Nobody would take a
  copy" reads as *no peer was available*, which is the one cause that gets better on its own, so
  the operator waits. Being over the cap never gets better.

  The reason existed and was thrown away twice. `Cluster::push_blob` received
  `BlobDeclined { reason }` — *"268517748 bytes is over the limit"* — and logged it at
  `tracing::debug!`, invisible at the default level, while the decline that arrives **after** a
  transfer (wrong bytes, a retry, genuinely transient) was at `warn`. Backwards in severity. Then
  `Ok(false)` dropped the sentence, and `Peers::replicate`'s `Option<NodeId>` dropped everything
  else.

  `Pushed::{Stored, TooLarge { size, limit }, Declined { reason }}` now, and `TooLarge` is decided
  **before a session is opened**: every node enforces the same cap, so it is a fact about the blob
  rather than about the peer, and dialling one to be told it both wastes a round trip and returns
  an answer that reads like somebody's opinion. `Replicated::{To, NoPeer, Refused { why,
  permanent }}` carries it out to the supervisor, which now says:

  ```
  WARN  checkpoint is on this node only and no retry will change that
        node=abe12bab
        reason=the blob is 268517749 bytes and the protocol moves at most 67108864 in one
               exchange, so no node in this fleet can take a copy
  ```

  And `NoPeer` — a fleet of one, with nobody to ask — dropped to `debug`, because warning every
  turn about the ordinary state of a single-node fleet is how a log gets ignored.

  Three `bool`-shaped answers were replaced in one session: `RepoReach` (seventy-five, re-read
  here), `Adopted`, and this pair. **The shape to recognise is a function that decides something
  with a reason and returns whether rather than why** — the sentence downstream then has nothing to
  say, and somebody writes a plausible guess into it that is wrong in exactly the cases that matter.

- **Building the first half of ADR-0061: what an archive workspace has to become.**

  §1 and §2, built in session seventy-eight. The shape that made it small is that **nothing
  downstream learns about archives at all**: `ensure_archive_mirror` leaves an ordinary bare mirror
  under `repos/`, and from there `prepare`, the `base..HEAD` bundle, the patch, `restore`'s
  `merge-base` reasoning, replication and migration are untouched. The test that proves it is the
  one that calls `prepare` on an archive-derived mirror and reads the agent's files out of the
  worktree.

  Four things were not obvious until the code was written:

  **An archive's mirror is immutable and must not be refreshed.** `ensure_mirror`'s warm branch
  fetches `origin` and treats failure as survivable — correct for a clone. An archive's origin was
  a scratch directory deleted at the end of the unpack, and its *name is the hash of its own
  bytes*, so there is nothing a fetch could bring. Left alone it fails on every start, survivably,
  logging `mirror refresh failed` about the one mirror in the system that cannot be stale.

  **And an archive is never cloned into existence.** Falling through to `git clone archive:<hash>`
  would be a confusing failure at the far end of a start. `ArchiveNotAcquired` is named instead,
  which is `Restorable`'s argument reused: the ordering mistake is a caller's, and a node that
  quietly carried on would build a workspace out of nothing.

  **`tar czf x.tar dir` archives the directory, not its contents.** Both spellings are things an
  agent will produce, and the second unpacks to a single child holding everything. A mirror built
  from that is a repository whose only tracked entry is a folder — every mechanism downstream is
  happy, and the agent finds its files one level deeper than it left them. The kind of wrong
  nobody goes looking for, so `single_child_dir` descends exactly when there is one entry and it is
  a directory.

  **The acquisition goes before the checkout lock.** `drive` takes `hold(run)` for as long as the
  workspace is being built, which is right for a worktree and wrong for a transfer: a fetch and an
  unpack is seconds to minutes, and the checkout sweep and `offload rm` would block on it. Nothing
  in the acquisition touches the checkout.

  Walked end to end on one daemon against the motivating case in miniature — a directory that is
  not a repository, holding notes, docs, a config file and a 2 MB backup:

  ```
  $ offload run --repo /tmp/o9/punch-out --queue "do the thing"      # one command earlier
    alpha        /tmp/o9/punch-out is here, but it is not a git repository

  $ tar -cf chosen.tar CLAUDE.md docs master-shop/conf               # the agent chooses
  $ offload run --archive chosen.tar "do the thing"
  archive 4f138a26b289  (10240 bytes)
  ```

  The agent's own `find` inside the worktree returned exactly the three chosen paths and not the
  2 MB backup; the run completed with one commit on its run branch; the mirror is
  `archive-4f138a26-baac4b01dd1240ee.git`; and `/tmp/o9/punch-out` still has no `.git` in it, which
  is §2's *never in the submitter's directory* checked rather than assumed.

  The over-cap refusal was walked with `MAX_BLOB_BYTES` lowered to 1 MiB: refused at
  `StoreArchive`, before the run exists and before any bytes are stored, naming both numbers and
  what to do — *"it has to be a smaller selection, not the whole directory"*. That is §4's *say the
  cap before the expensive step* met at the first place that can, and it needs no peer, because
  every node enforces the same limit.

- **ADR-0061 §3, and the two defects only two daemons could find.**

  §3 says the archive moves *once per node per run* and the per-turn checkpoint carries only what
  changed. The cadence half is true by construction — `Checkpoint::blobs` is transcript, bundle and
  patch, and the archive is none of them — so the build was a walk, and the walk found two things a
  single daemon never could.

  **The first: `replicated` was a false promise.** Alpha submitted an archive run, ran to turn 7,
  and `offload ps` said `SAFE: replicated`, whose meaning is *this survives alpha going away*.
  Bravo held every checkpoint blob and **not** the archive, because replication moves
  `checkpoint.blobs()` and the archive is not one of them. Killing alpha:

  ```
  INFO  reassigning        reason=HolderDead { absent: Millis(18100) }
  INFO  reassigned         name=bravo starting=starting now
  INFO  fetching an archive workspace archive=e7a0e262
  ERROR run failed  error=agent: fetching archive workspace e7a0e262: … no peer could supply
                           the blob: no route to 8633a994: 127.0.0.1:47821: timed out
  ```

  Twelve turns of work, on a run the operator had been told was safe. Every other part behaved
  perfectly: the detector, the reassignment, the epoch, the fetch, and the refusal's own wording.
  The fix is to replicate the archive *with* the checkpoint — not by adding it to
  `Checkpoint::blobs`, which would make that method lie and re-offer it every turn as part of the
  checkpoint, but by extending the set `replicate_now` pushes. It makes `is_durable` honest for
  free, which is why it belongs there: `Mesh::replicate` is all-or-nothing per peer, so a peer that
  could not take the archive is never recorded as a replica and the column stops promising what the
  fleet cannot do. §3's *once per node per run* still holds, because the second offer of an archive
  a peer already holds is declined as `already held` before any bytes move.

  **The second, found only because the first was fixed:** with the archive now on bravo, the
  migration got one step further and failed differently.

  ```
  INFO  unpacking an archive workspace archive=e7a0e262…
  ERROR run failed  error=workspace: git worktree add … afca1dde… failed (128):
                           fatal: invalid reference: afca1dde7781a130fd70969912c9b0bbc779d7b7
  ```

  §2 has the receiving node `git init` an archive that is not already a repository — and a commit
  hashes its tree, its parents, its message **and its two timestamps**. So alpha and bravo unpacking
  byte-identical archives produced *different* base commits, and the checkpoint bravo held named
  alpha's. The content-addressed archive gave the same bytes and not the same history, which is the
  one thing everything downstream assumes.

  So the repository built from an archive is content-addressed the way the archive is: fixed
  identity, `GIT_AUTHOR_DATE` and `GIT_COMMITTER_DATE` pinned to a constant, fixed message, and
  `--initial-branch` pinned because `init.defaultBranch` is global config and two nodes disagreeing
  about `main` versus `master` is the same failure by a slower route. **That commit id is a wire
  format in all but name** — a node that upgrades mid-run must still find the base its peer's
  checkpoint names — so a test pins the literal digest, not just cross-node equality.

  After both, the same walk end to end: killed on alpha at turn 5, running on bravo at turn 13,
  one unbroken branch reading `the archive this run started from` → turns 1–5 → turns 6–12, with
  the archive's files and the agent's own beside them. That is *close the laptop, the agent keeps
  working on the desktop* for a directory that is not a git repository and has no origin to clone.

  The reusable part is not about archives: **anything a second machine has to reproduce must be a
  function of its inputs alone**, and the two things that leak in are the clock and whatever is in
  somebody's global config. Both leaked here, in one six-line block, and neither is visible on one
  daemon.

- **The acquisition report: what a subset workspace needs and a cloned one does not.**

  ADR-0061 names this twice, and its §4 amendment promoted it from a nicety to the thing that makes
  the design's own failure diagnosable. The reasoning is worth keeping because it is not obvious
  that a report is load-bearing at all.

  A cloned repository needs no line saying what is in it: the ref names a commit, the commit names
  a tree, and anybody with the repo can go and look. An **archive** is a subset an agent chose,
  nobody else holds a copy of that selection, and the failure this design has is *a run dying for
  want of a file that is sitting on the operator's disk*. Without a record of what travelled, that
  is indistinguishable from a workspace that never arrived — and the two want opposite responses.

  Walked by staging exactly that: an agent that stops when `master-shop/conf/settings.ini` is
  absent, and an archive that deliberately leaves `master-shop/` out.

  ```
  submitted   archive:3a7f8fda…
  archive     3a7f8fda
              2 file(s), 16 bytes — CLAUDE.md, docs/
  workspace   /tmp/oq/alpha/worktrees/01a0916da595…
  …
  cannot continue: master-shop/conf/settings.ini is missing
  failed: the agent stopped without reporting a result
  ```

  Two lines apart, a reader has the answer: the selection was wrong, not the machinery. That is the
  whole of what this is for.

  Three decisions in it:

  **It lives in the run's log, not only the node's.** The question is asked from anywhere and
  usually later — often on a different machine, after a migration — and `tracing` output is on one
  disk.

  **Top-level entries and a count, never every path.** An agent selecting 12 MB out of 6.5 GB may
  still bring thousands of files, and a log line nobody reads to the end reports nothing. The roots
  are what the agent actually *chose*, which is the decision under review.

  **Only when bytes actually moved.** `ensure_archive_mirror` answers `Option<Acquired>` and the
  warm path answers `None`, so the caller *cannot* log an acquisition that did not happen. Measured:
  a second run against the same archive has no `archive` line at all, which is §3's once-per-node
  property read from the reporting side. The alternative — logging it every time — would put a
  second "what travelled" answer in the log of a run whose workspace had not changed since the
  first, and a reader has no way to tell a re-acquisition from a repetition.

  And the counting is done over the **unpacked tree** rather than the tar's own listing, because
  what a person needs is what is *there*: a tar may carry entries that do not land, and the descent
  into a single root changes what every path means. `.git` is skipped, since the synthetic history
  is not part of what the agent sent and counting it would make the number disagree with the
  archive they built.

- **The teardown an operator types was the one path ADR-0055's rule had never been applied to.**

  `offload rm` answered, unconditionally:

  ```
  worktree removed (the run's branch and commits are kept)
  ```

  True, and the reassuring half. The sweep asks `holds_uncommitted` and **keeps** such a checkout;
  `supersede` asks and **moves it aside**; the path a person types asked nothing. And it is the path
  most likely to meet the case: a `failed` run is terminal *and* resumable, so its worktree is the
  one holding edits past the last turn boundary, and `offload rm` is one keystroke from `offload
  resume` in anybody's history.

  Measured — a modified tracked file and an untracked one, in a completed run's checkout:

  ```
   M base.txt
  ?? notes.txt
  ```

  removed, and `offload ps`, `offload logs` and `offload audit` between them said nothing about
  either. Nothing else anywhere holds them: not the run branch, not the checkpoint, not the bundle,
  and `git fsck` finds nothing because none of it was ever an object.

  Now:

  ```
  worktree removed — its branch and commits are kept, but it held 1 modified, 1 new that nothing
  else has a copy of, and that is gone
  ```

  Three things about the shape:

  - **Asked before the teardown**, for `supersede`'s reason — the removal is what makes the question
    unanswerable, so afterwards there is nothing left to ask.
  - **Not a refusal.** Whoever typed it is entitled to the disk back; the command is manual for
    exactly that reason. What they are owed is being told, once, while they can still act on it.
  - **`commits_ahead` is deliberately left at zero** (`parse_porcelain` does not set it), so the
    count is of what teardown *destroys* and never of the commits it keeps. A line that cried wolf
    on every ordinary `rm` — a completed run whose agent committed everything — would be noise on
    every teardown in the fleet, which is why the clean case still says only the old sentence.

  `Removal::Removed { discarded: Option<String> }`, in the words `offload ps`'s WORKSPACE column
  already uses. The sweep's arm names `discarded: None` explicitly rather than ignoring it — its
  `reclaimable` guard has already refused a checkout holding anything uncommitted — so a future
  `Some` there is a compile error and not a silent reclamation of somebody's only copy.

*The five entries below were backfilled in session ninety-one — four from `docs/sessions.md`, and
the last from the body of the commit that added its rule (`94c701f`), which is the only record of
it.*

- **A fact the record cannot carry is not a fact nothing can carry.**

  Session fifty-one (commit `8ad0aa1`). `reclaim_departed_checkouts` removed a departed run's
  worktree and wrote a `tracing::info!` on a machine nobody is logged into. The stated blocker:
  `cleanup` notes `workspace: "removed"`, `RunProgress::workspace` is one fleet-agreed value with
  one writing leg, and the sweep runs on the leg that has *lost* the run — so `update_stats` drops
  the write, correctly (a losing leg describing a worktree is session seventeen's bug). Two
  handoffs read that as "so nothing can say it", and one went looking for a new gossiped fact.
  What happened to a directory is a fact about **one disk**: nothing to merge, no owner to
  arbitrate. The per-node audit log exists for exactly that, so `AuditEvent::Reclaimed { run, why }`
  — no migration (the variant is stored as JSON beside a `kind` string), no wire bump — with
  `Reclamation::{MovedOn, NobodyWaiting}` naming which of the sweep's two doors it was. Measured on
  two daemons with the timer at fifteen seconds: handover at 18:21:03, the reclaim at 18:21:18, and
  in `offload audit` *"reclaimed this run's checkout here: another node has run it since, so the
  work went with it"*, the run's own record untouched.

- **`git worktree add`'s failure wording says whether anything raced, and the two causes need
  opposite fixes.**

  Session fifty-three (commit `e085088`, ADR-0051). `WorkspaceManager::prepare` passes `-B` and
  **no `--force`**, so git's `die_if_checked_out` fires first and says `'X' is already used by
  worktree at 'X'` — what a *leftover* checkout produces. The logged sentence was `cannot force
  update the branch 'X' used by worktree at 'X'`, from `create_branch`, one check later — which only
  the loser of a genuine race reaches, before the winner registers. Reproduced outside this
  codebase on git 2.55: 200 forced pairs of concurrent `worktree add` at one path gave 17 of the
  second wording and 182 of the first. So the symptom was never a stale worktree: it was two
  `prepare` calls at once, two legs past `Supervisor::register`, both entitled to the same epoch.

- **"It is still in the reflog" is not a place you put something.**

  Session sixty-two (commit `aae378d`), ADR-0053's residual. When a restore's `reset --hard` moves
  past commits this checkout had and the bundle does not, the epoch has decided which leg is the
  run and the reset is right — the question is what becomes of the commits. The residual said they
  were "named in the log and kept by the reflog"; an unreachable commit's reflog entry expires (30
  days by default, sooner under an automatic `gc`), and a hash in a log line needs somebody to know
  to look and still have the log. They get a **ref** now, `refs/offload/left-behind/<run>/<short>`
  — safe because `ensure_mirror` is not a `--mirror` and `fetch --prune` touches only
  `refs/remotes/origin/*` — and `LeftBehind { commit, kept_as }` says whether writing it worked.
  Tested by diverging two legs on one base and asserting `git show <ref>` still has the work after
  the reset; red with the `update-ref` removed.

- **A hand-written list of every variant goes stale in silence.**

  Session sixty-four (commit `df4305e`). `AuditEvent::Rescued` was added with no line in
  `every_kind_round_trips` — not a failing test, but a variant nothing had ever encoded, and
  encoding is the only thing between the audit log and being unreadable. The guard now counts
  `kind_name`'s arms from the enum's own source (`no_clock.rs`'s trick in a smaller place), verified
  by inventing a variant and watching it fail. What it does not reach — a new arm of `Reclamation`,
  `Rescue` or `Attempt` — is said in the test.

- **…and the sweep caught the new fixture doing it too.**

  Session sixty-four, a follow-up commit (`94c701f`) with no `docs/sessions.md` paragraph of its
  own. The `None` sweep, re-run over ADR-0055's *new* test: its fixture set `bundle: None`, so the
  other rescue on that path — the ref naming commits a `reset --hard` moves the branch off — could
  never fire in it, and the case an operator reading `offload audit` actually meets (both rows for
  one run, in one resume) was unstaged. Behaviour was already right and the new assertion passed on
  the first run; a build with `if diverged && false` showed it load-bearing — without it the report
  read `rebuilt from bundle; … nothing uncommitted, so it was removed` and said nothing about the
  commits it walked past.

- **A directory the sweep reads must belong to one node.**

  Session ninety-two, ADR-0074. Moving checkouts to `~/offload` by default meant three walk daemons
  on the laptop would share one directory. `WorkspaceManager::checkouts` lists the directory, and
  the sweep reclaims entries that no local run needs, so each daemon would have removed the others'
  live checkouts as departed. Found by reading the sweep before changing the path, not by a walk.
  `statedir::claim_checkouts` writes the node id with `create_new`, and a second node is refused at
  start ("holds the checkouts of another node … Set `[workspace] dir`"). Walked with a throwaway
  node against the one that owned `~/offload`. The default is applied in `offloadd`'s `main`
  (`with_readable_checkouts`), because `Config::default()` is what the tests build.
