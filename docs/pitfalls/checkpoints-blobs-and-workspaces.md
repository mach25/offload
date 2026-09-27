# Checkpoints, blobs and workspaces

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/checkpoints-blobs-and-workspaces.md`, same order.

- **Mid-turn is not a safe checkpoint.** State is in the tool, not the transcript. At a drain
  deadline the honest options are wait or lose the turn — never snapshot anyway.
- **`RunProgress::workspace` is refreshed at every turn boundary, not at every checkpoint.** Tying
  it to the capture is two turns stale under `every_turns = 3` and permanently wrong at `0`. A
  field written twice in a run's life is wrong in between — it read `preparing` all night.
- **`Selection::summary` needs three reasons, not two.** A file dropped for the *total* budget is
  not one that was too large; the message named the wrong setting. That line is the whole of what
  the operator is told about work that did not travel.
- **A status decides where work *goes*, never who is worth *asking*.** `fetch_blob` ordered, not
  filtered: hint first, then live peers, then everyone else. The moment a checkpoint is needed is
  the moment somebody went quiet, and `Suspect` exists to be argued with.
- **A checkpoint stops being one when the run stopped by decision.** Nothing fetches a checkpoint
  for a `Completed` or `Cancelled` run, so the collector kept the biggest blob for ever — zero is
  the right number. `Run::resumable_checkpoint` over `RunState::may_resume_later`, whose arms are
  written out so a new terminal state must decide. `Failed` is the exception: it reopens.
- **…and `is_terminal` is the wrong question in a report for the same reason.** A failed run is
  terminal *and* reopens, so it is exactly the row `here only` was written for.
  `Checkpoint::is_durable(Option<NodeId>)` — a run with no holder has nothing to discount, and
  `replicas` never contains the node that took the checkpoint.
- **"Deliberately manual" means nobody runs it.** `blobs::collect_garbage` was careful,
  property-tested and called by nothing. A transcript is the whole conversation so far, so waste is
  quadratic. On a tick, with an hour of grace, because a blob is written *before* the row naming it.
- **Uncommitted work is the valuable part.** Untracked files count; `git status` without
  `--untracked-files=all` will not show a newly written source file.
- **A worktree that is still here is not necessarily the current one.** Nothing removes a worktree
  when a run leaves. The worktree records the turn it holds and `adopt` answers `Current |
  Superseded | Absent`; **unknown is not current**, and a superseded checkout is moved aside *if it
  holds anything uncommitted* — which is a separate question with its own answer (ADR-0055), asked
  before the move because the move makes it unanswerable.
- **A directory-name list is a guess about somebody else's repository, so it loses to what that
  repository tracks** — asked per directory, so `build/target/debug/x.o` is still caught. Residual:
  a directory the agent creates from scratch, where there is nothing to ask.
- **Unknown is not none, in a collector.** A run row this build cannot decode has *unknown*
  references; `collect_garbage` fails the whole pass rather than collecting what it points at.
- **Never `clone --mirror` a repo cache.** Its `+refs/*:refs/*` refspec makes the next
  `fetch --prune` delete every `offload/run-*` branch. Upstream refs belong under
  `refs/remotes/origin/*`.
- **A local-path repo pins a run to one machine.** `RepoSource::portability()` is an eligibility
  fact; discovering it at resume time means the migration already failed.
- **A capture whose transcript holds no conversation must be refused, not recorded.** The agent
  writes the file behind its own event stream by a *variable* amount — two runs captured at the
  same turn 1 stored 22,953 bytes with the conversation in it and 17,087 bytes with none of it.
  Resumed from the empty one the agent has lost its own half, says "No response requested.", makes
  no tool call, and the run reports **`Completed`**. `checkpoint` now refuses it; the existing
  check asked only whether the *file* existed. A failed capture is not a failed run — the next
  boundary succeeds.
- **A run held about one turn behind in the transcript is normal, not a symptom.** Measured across
  a healthy five-turn run: turn 2 with 1 message, turn 3 with 1, turn 4 with 2, turn 5 with 3.
  Only *nothing* is fatal. A warning on the lag fires at every boundary of every run.
- **The capture at a turn limit is the one capture with no retry, so it must wait for the flush
  before it stops the agent.** "A failed capture is not a failed run — the next boundary succeeds"
  is true of every caller but the turn limit, which captures at the boundary it *stops* on.
  Measured with a real agent: **two of three `--max-turns 1` runs ended with no checkpoint at
  all** — no `checkpoint` line in `offload explain`, a dash under SAFE — while `note_tokens`, which
  reads the same file later, filled in TOKENS beside it. The failure is **binary, not gradual**:
  every transcript that had not caught up was *exactly* 24,188 bytes with zero assistant rows,
  every one that had was ~30KB with two or three. And it is **not a lag that exit resolves** — a
  `SIGTERM` landing inside the window loses the message outright, and those files still hold zero
  assistant rows now. So the order is **wait, then stop, then capture** (`await_transcript`, 3s,
  polled): 8/8 kept afterwards against 1/3 before, with the timeout never firing.
- **The checkout sweep's guards were on the far side of a subprocess.** Every door in
  `reclaim_departed_checkouts` — not the holder, no live agent, has moved on — was read, then a
  `git status` was awaited, and only then was `git worktree remove --force` run. A run picked back
  up inside that 3ms window kept none of it: **40 of 40** attempts deleted the checkout out from
  under a run this node had just assigned to itself. Not a photo finish. `reclaimable` is asked
  twice now, and the second answer is the one that decides. The sibling path is safe for a reason
  worth knowing rather than by luck: `reclaim_occurrence` goes through `cleanup`, which reloads the
  run and refuses a non-terminal one — and `release` clears the agent handle *before* the terminal
  write, so a terminal run never has a live agent.
- **…and `cleanup`'s own guard is already next to its effect**, which two handoffs suspected it was
  not. It loads the run, refuses a non-terminal one, and calls `remove` with **no `await` in
  between**, so a second check would sit beside the first. It is the await that makes a gap, not
  the effect being dangerous. Checked, and not a finding.
- **A fact the record cannot carry is not a fact nothing can carry.** The checkout sweep removed a
  worktree and wrote a `tracing::info!` on an unattended machine, and the residual explaining why
  was right about the record and stopped one step short: `RunProgress::workspace` is one
  fleet-agreed value with one writing leg, the sweep runs on the leg that has **lost** the run, so
  `update_stats` correctly drops the write — and every other field on the record has that same
  shape, which is what two handoffs read as "so nothing can say it". What happened to a directory
  is a fact about **one disk**, with no second opinion anywhere to merge with, so it belongs in the
  per-node log that already exists for exactly that (schema v6): `AuditEvent::Reclaimed`, read by
  `offload audit <run>`. **Ask what the fact is a property of** — the same question ADR-0042 asked
  to put `let_go_by` inside `Pending` rather than beside it. And it carries **which of the two
  doors** it went through, because "your worktree is gone" without *the run moved* or *nobody was
  going to read it* is the decision having discarded its reasoning. Where both doors are open —
  a rule's occurrence that started here and finished on a peer — `MovedOn` wins, since where the
  work went is the more useful of two true things; that precedence is a claim about line order, so
  it is written out in a test rather than left to the `match`.
- **`git worktree add`'s failure wording says whether anything raced, and the two causes need
  opposite fixes.** `prepare` passes `-B` and **no `--force`**, so on git 2.55: a worktree added
  earlier, or a directory renamed away and not pruned, both give `'X' is already used by worktree
  at 'X'` — a leftover, fixed by pruning or superseding. **`cannot force update the branch 'X' used
  by worktree at 'X'` is the concurrent one**, from `create_branch` after `die_if_checked_out` has
  already passed: `worktree add` is a read-decide-write, and the loser gets through the checked-out
  test before the winner registers. Measured over 200 forced pairs at one path: 17 the second
  wording, 182 the first, 1 a torn `worktrees/<name>/commondir`. That distinction is what turned
  session fifty-two's unexplained "two workspaces for one run" into two legs past
  `Supervisor::register` (ADR-0051) rather than something upstream in placement.
- **A guard cannot be put inside somebody else's program.** Both teardown doors were guarded
  correctly and both were still open: the effect is `git worktree remove --force`, ~2ms of
  subprocess, and a rebuild landing in it lost **10 of 10** times (0 of 50 outside it). The answer
  is a lock, and its shape is that the four mutators live on the `CheckoutGuard`, taking the run id
  from it — so the compiler asks. `try_hold` for the sweep, since `hold` deadlocks a tick behind a
  bundle fetch; and `cleanup` takes the hold *before* the load, because taking a lock is an await.
- **A checkpoint taken with no peer reachable stays `here only` for ever.** `replicate` has one
  caller (`checkpoint`) and no retry, so a peer arriving later fixes nothing — measured: four
  checkpoints taken alone were still `here only` after the peer was up and both nodes called each
  other `alive`, and only the *next* checkpoint replicated. With a peer, replication is 1.3–47.3 ms
  for blobs up to 11.5 MB and **30 of 30** landed, so durability is never a race — it is a question
  of whether anybody was there at the time, and a failed run has no next checkpoint to fix it.
- **A rule with two doors and one guard is a rule about one door.** `start_run` fetched the blobs
  a checkpoint names before starting an agent from it; `resume` — which is `offload resume` *and*
  the recovery tick — built the same `Start::Resume` and went straight to `launch`. So a run
  resumed on a node the checkpoint was never replicated to answered **`resumed <id>`** and then
  died 12ms later inside the restore with `state store: blob bf5035b4… is not stored here`,
  leaving the run `failed` at a burnt epoch. Nothing fetched, nothing logged. `Restorable` is the
  fix: `Start::Resume` cannot be spelled without it and `ensure_blobs` is the only thing that
  makes one, so the compiler asks. **Migration to a node that has not already been replicated to
  was the commonest thing this system claims to do, and no walk had ever done it** — every one had
  the blobs there already.
- **…and the store is asked twice, the second answer deciding.** A `fetch` that returns `Ok`
  without leaving the bytes here is otherwise indistinguishable from one that worked, and the
  place that finds out is `restore_workspace`, with a worktree half built.
- **"The branch is here" is not "the work is here" (ADR-0053).** The bundle used to be applied
  only when the run branch was *absent* from the mirror — but a branch of that name is there
  whenever this node has ever run a leg, and nothing keeps it current. A run that ran on alpha,
  migrated, committed on bravo and came back kept **four of eight files**, with `offload logs`
  saying `re-checked out, patch reapplied` over a bundle that was replicated, named, and never
  opened. An earlier pass had the two branches intersecting in **exactly one commit**, the base.
  `restore` asks `git merge-base --is-ancestor` now: apply unless this checkout already contains
  the bundle's tip. The old guard's *hazard* was real — `reset --hard` onto a branch that is ahead
  walks a live run backwards — it was the *question* that was wrong.
- **…and a leg that dies between `prepare` and `restore` poisons that node for ever.** `prepare`
  creates the branch at the base commit, so a failure in the window left it there, and every later
  resume on that node then skipped the bundle. Not a race you lose once — a state you stay in.
- **The test that should have caught it staged it and left the bundle out.**
  `a_worktree_from_an_earlier_leg_is_not_mistaken_for_the_current_one` is *the* run-comes-back
  test and sets `bundle: None`, putting the far leg's work in an uncommitted file — so it walked
  the patch and never the commits. **When a fixture sets a field to `None`, ask what that field
  being `Some` would have done.**
- **A same-node resume normally never reaches `restore`.** `adopt` reads the worktree's turn
  marker, and after a `kill -9` that marker sits at the last *capture* — so it equals
  `checkpoint.turns`, counts as `Current`, and the answer is `adopted in place`. Anything about
  what `restore` decides therefore needs the **branch to outlive the worktree**, which is the case
  the design already has a name for. Walked: marker at turn 5, branch at `L1 t8`, checkout
  removed — `the commits here already held this checkpoint`, all eight commits kept, against a
  build with the `--is-ancestor` check forced false that kept five and said so.
- **`list_runs(false)` omits exactly the runs a last-copy pass exists for.** A `Failed` run is
  *terminal*, so the flag that means "skip terminal rows" skips the ones whose checkpoint decides
  whether the work survives. Same trap as the `is_terminal` entry above, met in a new function and
  caught by its own test at the first run. `resumable_checkpoint()` is the filter that is right —
  its arms are written out, so `Completed` and `Cancelled` are refused and a new terminal state has
  to decide.
- **A departing node pushes the last copy before it decides what to strand (ADR-0054).** Walked:
  alpha alone, run fails at turn 4 with its only checkpoint here, bravo starts *after*, then
  `offload drain` — before, `nothing to hand over` and the run stayed `failed`/`here only` on the
  machine that was leaving; after, `copied 1 checkpoint(s) nobody else had to a peer` and the run
  is **running on bravo at turn 5**. The fact `fleet_could_start` reads was one nothing on the way
  out had ever tried to improve.
- **"It is still in the reflog" is not a place you put something.** A `reset --hard` past commits
  this checkout had leaves them unreachable, and an unreachable commit's reflog entry expires — 30
  days by default, sooner if an automatic `gc` runs — while the hash went only into a log line
  somebody has to know to read. Both halves are weaker than they sound. `restore` writes
  `refs/offload/left-behind/<run>/<short>` first, which is the same answer `supersede` gives a
  checkout it moves aside, in the namespace `fetch --prune` cannot touch (the repo cache is
  deliberately not a `--mirror`). Best effort, and the run's log says when it could not.
- **A rescue that destroys the evidence for its own necessity keeps everything for ever
  (ADR-0055).** `supersede` renamed a superseded checkout unconditionally *and* deleted the copy's
  `.git` file — so from that moment nothing could tell it from the one holding somebody's only
  mid-turn edit. Measured: `git status` in a rescued copy answers `fatal: not a git repository`,
  and three legs of one run left three full copies of the tree that nothing would ever remove. So
  `holds_uncommitted` is asked **before** the rename, and a clean checkout goes through `remove`
  instead — its commits are on the run branch. `None` still keeps the copy. **Ask what a decision
  will still be answerable with afterwards**, not only whether it is right now.
- **…and a rescue is a fact about one disk, so it is a per-node audit row.** Both rescues on that
  path — the checkout, and the commits under `refs/offload/left-behind/<run>/` — were a
  `tracing::warn!` and a line in the *run's* log, which is served by the run's **holder** while
  the leg doing the rescuing is the leg that lost it. `AuditEvent::Rescued` and a third
  `Reclamation::Redundant` door, `Reclaimed`'s reasoning applied to its counterpart. The row needs
  no path: both names are derived from the run id.
- **A hand-written list of every variant goes stale in silence.** `every_kind_round_trips` covers
  the audit log's encoding, which is the only thing between that log and being unreadable — and a
  variant added with no line in it is not a failing test, it is a variant nothing has ever encoded.
  `Rescued` was that variant. The arms of `kind_name` are the enumeration now, counted from the
  enum's own source (`no_clock.rs`'s reason, smaller). It does **not** reach a new arm of
  `Reclamation`, `Rescue` or `Attempt` — those are still hand-enumerated and unchecked, which the
  test says out loud.
- **…and the sweep caught the new fixture doing it too.** The test for the redundant-checkout arm
  set `bundle: None`, so the *other* rescue on that same path — the ref naming commits the reset
  moves past — could never fire in it, and the case an operator reading `offload audit` actually
  meets, both rows for one run in one resume, was unstaged. Behaviour was already right; it passed
  first run, and a build with `if diverged && false` proves the assertion is load-bearing. **The
  `None` question is worth asking of a fixture you wrote five minutes ago, not only of an
  inherited one.**
- **The blob plane moves whole blobs, in memory, with a 512 MiB cap — and ADR-0061 wants 6.5 GB
  through it.** Measured on two daemons: a 256 MiB bundle replicates in **6.36s** over loopback
  (~40 MiB/s) with peak RSS **308 MB sending and 298 MB receiving**, from a 33 MB baseline, because
  `Blobs::get` returns `Vec<u8>` and `store` takes one. Killing the receiver mid-transfer left
  **363 bytes** — the previous turn's transcript blob — and nothing partial, because `recv_bytes`
  fills one buffer and the store writes temp-then-rename. So acquisition is **not resumable and
  there is nothing to resume from**, which is correct rather than sloppy and is a wire change to
  alter. Before designing anything that puts a large tree in a blob, read the numbers rather than
  the trait.
- **A checkpoint too big to replicate said "nobody would take a copy".** That reads as *no peer
  was available*, which gets better on its own; being over `MAX_BLOB_BYTES` never does. The peer's
  actual sentence went to a `tracing::debug!` while the refusal that arrives *after* a transfer —
  the transient one, wrong bytes and a retry — was at `warn`: exactly backwards. `Pushed::{Stored,
  TooLarge, Declined}` now, with `TooLarge` decided **before dialling** from the bytes in hand,
  since every node enforces the same cap and that makes it a fact about the blob rather than about
  the peer.
- **…and `Peers::replicate` answered `Option<NodeId>`, so every reason collapsed into one.** A
  fleet of one, a refusing peer, a transport error and an unmovable blob were the same `None`, and
  the fleet-of-one case — the ordinary state of a single-node fleet — warned every turn.
  `Replicated::{To, NoPeer, Refused { why, permanent }}`. Third instance of this shape in one
  session, after `Adopted` and session seventy-five's `RepoReach`: **a `bool`-shaped answer throws
  away the measurement the sentence needed.**
- **An archive mirror is immutable, so it is never refreshed.** `ensure_mirror`'s warm branch
  runs `fetch --prune origin` and treats failure as survivable — right for a clone, wrong for an
  archive, whose origin was a scratch directory that no longer exists and whose *name is the hash
  of its own bytes*. Left alone it would fail every call, survivably, and log `mirror refresh
  failed` about a mirror that is as current as it will ever be. Asked before the refresh, not
  after.
- **…and an archive is never *cloned* into existence either.** There is nothing to clone from,
  which is the whole reason ADR-0061 exists. `ensure_mirror` refuses an archive it does not
  already hold with `ArchiveNotAcquired` rather than falling through to `git clone archive:…`:
  the bytes are fetched and unpacked by `ensure_archive_mirror` first, which the supervisor calls
  before it takes the checkout lock. Named rather than papered over, for `Restorable`'s reason —
  a node that quietly carried on would build a workspace out of nothing and fail somewhere less
  obvious.
- **`tar czf x.tar dir` archives the directory, not its contents**, and both spellings are things
  an agent will produce. Unpacked, the second leaves one child holding everything — and a mirror
  built from it is a repository whose only tracked entry is a folder. Every mechanism downstream
  is perfectly happy with that, and the agent finds its files one level deeper than it left them.
  `single_child_dir` descends exactly when there is one entry and it is a directory; anything else
  is used as it came out.
- **The archive acquisition is taken *before* the checkout lock.** A fetch and an unpack is
  seconds to minutes on a large archive, and holding `hold(run)` across it would block the checkout
  sweep and `offload rm` for the whole transfer. Nothing in the acquisition touches the checkout —
  it builds a mirror, which is per-archive and race-safe by scratch-then-rename.
- **A synthetic history must be content-addressed, or the archive cannot migrate.** ADR-0061 §2's
  `git init` embeds *when it ran and who ran it*, so two nodes unpacking **identical bytes** built
  different base commits — and the peer that took a migrated run over answered `fatal: invalid
  reference <the other node's base>`, with a correct checkpoint and a correct bundle rooted in a
  history that did not exist there. Fixed identity, fixed `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE`,
  fixed message, and `--initial-branch` pinned — the last because `init.defaultBranch` is somebody's
  global config and `main` versus `master` is the same failure by a slower route. **The commit id
  is a wire format in all but name** and is pinned by a test: a node that upgrades mid-run must
  still find the base its peer's checkpoint names.
- **A checkpoint is not durable if the workspace it needs is not.** `Checkpoint::blobs` is what the
  checkpoint is made *of*; what replication must move is what a peer needs to **materialise the
  run**, and for an archive workspace that includes the archive. Measured: bravo held every
  checkpoint blob, `offload ps` said `replicated`, alpha was killed, bravo took the run over
  exactly as designed and failed at turn 12 with *"no peer could supply the blob"* — twelve turns
  lost on a run whose SAFE column had promised it would survive. The archive is pushed with the
  checkpoint now, which makes `is_durable` honest for free: `Mesh::replicate` is all-or-nothing per
  peer, so a peer that could not take the archive is never recorded as a replica.
- **A subset workspace needs a report a cloned one does not.** A repository's ref says what is in
  it and anybody can go and look; an archive is a selection nobody else has a copy of, so *"the run
  died for want of a file"* and *"the workspace never arrived"* are indistinguishable without one.
  `LogKind::WorkspaceAcquired` goes in the **run's** log rather than only the node's, because the
  question is asked from anywhere and usually later. Top-level entries plus a count, not every
  path: an agent selecting 12 MB of 6.5 GB may still bring thousands of files, and a line nobody
  reads to the end reports nothing.
- **…and it is written only when bytes actually moved.** A node that already holds the archive
  acquired nothing, and a line claiming otherwise would put a second *"what travelled"* answer in
  the log of a run whose workspace has not changed. `ensure_archive_mirror` answers
  `Option<Acquired>` — `None` for the warm path — so the caller cannot log an acquisition that did
  not happen. §3's once-per-node property, read from the reporting side.
- **The report counts the tree, not the tar.** What a person diagnosing a missing file needs is
  what is *there*, and the two differ: a tar may carry entries that do not land, and descending
  into a single root changes what every path means. It also skips `.git`, because the synthetic
  history is not part of what the agent sent and counting it makes the number disagree with the
  archive they built.
- **A teardown that reports only what survived is the reassuring half of a true sentence.**
  `offload rm` answered *"worktree removed (the run's branch and commits are kept)"* unconditionally,
  whatever the checkout held. A `failed` run is terminal **and** resumable, so its worktree is the
  one most likely to hold edits past the last turn boundary — and those have no copy anywhere: not
  on the branch, not in the checkpoint, not in the bundle, and `git fsck` finds nothing because
  none of it was ever an object. ADR-0055's rule (`holds_uncommitted` before the move) was applied
  on the sweep's path and on `supersede`'s, and never on **the path an operator types**. Measured:
  a modified file and an untracked one went, and `offload ps`, `offload logs` and `offload audit`
  between them said nothing about either. Asked **before** the removal, because the removal is
  what makes it unanswerable; not a refusal, because whoever typed it is entitled to the disk
  back. `Removal::Removed { discarded }`, with `commits_ahead` deliberately left out — committed
  work is on the run branch and survives.
- **A directory the sweep reads must belong to one node.** The checkout sweep reclaims whatever in
  its directory no run of *this* node's needs. A default shared across daemons (`~/offload`,
  ADR-0074) would have each one delete the other's live checkouts. Outside the state dir it is
  claimed by `.offload-node` and a second node refuses to start. And a default like that is applied
  by `offloadd` at start, never in `Config::default()`, which every test builds.
