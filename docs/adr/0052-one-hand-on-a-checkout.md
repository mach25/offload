# ADR-0052: One hand on a checkout — the lock is the type

**Status:** accepted · 2026-08-31 · closes the residual ADR-0051 and ADR-0050 both name ·
third of the three per-run exclusions, and the last one

## Context

Three things on a node change a run's worktree, and until now nothing stopped two of them
overlapping:

* **A rebuild** — `Supervisor::drive`, either arm. `Start::Fresh` calls `prepare`; `Start::Resume`
  calls `restore_workspace`, which asks `adopt`, may `supersede` what it finds, and then
  `prepare`s a replacement.
* **`offload rm`** — `Supervisor::cleanup`, an operator saying discard this checkout.
* **The checkout sweep** — `Supervisor::reclaim_departed_checkouts`, ADR-0020 §6's automatic half,
  on a tick with nobody watching.

Every one of them was guarded, and the guards were good. `cleanup` loads the run, refuses a
non-terminal one, and calls `remove` with **no `await` in between** — two handoffs suspected that
gap and it was not there. The sweep asks `reclaimable` **twice**, the second time on the far side
of its `git status`, because the first answer is stale by the time it is acted on; that fix was
worth 40 of 40 attempts.

What none of them could guard is the **inside of the effect**. `WorkspaceManager::remove` is
`git worktree remove --force`: somebody else's program, a few milliseconds long, deleting a
directory tree. A guard read before it is on the wrong side of it — the same rule as "a fence
after the effect is not a fence", one level down, where the thing that cannot be split is a
subprocess rather than a statement.

### Measured

A forced pair in a scratch repo, no daemon: `prepare` a worktree, write an uncommitted file into
it, then start `remove` and start a rebuild — `adopt`, falling through to `prepare` — a fixed
number of milliseconds later, sweeping that delay across the window.

| rebuild starts | outcome |
| --- | --- |
| 0ms into the removal | **adopted a checkout that was gone when `adopt` returned** |
| 1ms | **adopted a checkout that was gone when `adopt` returned** |
| 2ms and later | fine: the removal finished, `adopt` said `Absent`, the rebuild rebuilt |

10 of 60 passes lost, and **every single pass inside the window lost** — 10 of 10 at 0–1ms, 0 of
50 at 2ms or more. Not a photo finish: a ~2ms window, which is how long
`git worktree remove --force` takes on this machine, that the rebuild loses every time it lands
in. That is the same shape as the sweep's own 3ms `git status` finding, and it is why the answer
here is a lock rather than another check: there is no third place to put a read that is not
already inside somebody else's program.

The consequence is a `Workspace` handed to a run whose directory does not exist. `adopt` returns
`Adoption::Current`, `restore_workspace` reports "adopted in place", `install_transcript` skips
the install on the strength of that word, and the agent is spawned with a `cwd` that is gone.

## Decision

### 1. A per-run checkout lock, and the four mutators live on the guard

`WorkspaceManager::hold(run).await` returns a `CheckoutGuard`. `prepare`, `adopt`, `supersede` and
`remove` are methods **on the guard**, not on the manager, and they take the run id *from* it.

The compiler asks for the lock. That is the whole reason for the shape: a rule enforced by a doc
comment is a hope, and this one has to survive a caller nobody has written yet.
`--strict-mcp-config` sat in the pitfall list for two phases as a warning about a hole that was
open the entire time, and the difference was never that somebody forgot to read — it was that
nothing asked.

Taking the id from the guard rather than accepting a `run: RunId` beside a `&CheckoutGuard` closes
the one mismatch the weaker signature would allow: a guard for one run used to change another
run's checkout, which is a lock that reports success and protects nothing.

### 2. It is held across the *sequence*, which is why it is not an internal lock

`restore_workspace` asks three of the four in a row, and a lock taken inside each of them would
serialise every call and protect none of the gaps between them. `adopt` says "this directory is
current"; `prepare` acts on the answer; a removal between the two makes the answer false. So the
unit of exclusion is the caller's sequence, and the guard is a value the caller holds.

### 3. Only until the run is claimed, and no further

The guard is dropped at the end of `drive`'s workspace block, before the agent starts. Holding it
for the hours a run lasts would stop the sweep dead, and it is not needed: `register` claims the
run in `live` **before** `launch` spawns `drive` (ADR-0051), so by the time a rebuild touches the
disk, every teardown door's own guard already refuses.

That is what makes this a narrow lock rather than a second copy of the run state machine. It
covers exactly the gap between *the checkout exists on disk* and *the run is claimed in memory*,
and the two existing guards cover everything either side of it. Stated as the argument the sweep
now relies on: a rebuild that has started **holds** the checkout, so `try_hold` refuses; a rebuild
that has not started yet has already **registered**, so `reclaimable` refuses. There is no third
moment for one to arrive in.

### 4. The sweep does not wait; the operator's command does

`try_hold` for `reclaim_departed_checkouts`, `hold` for `drive` and `cleanup`.

The sweep has nothing to wait for — a checkout somebody is *rebuilding* is precisely one it must
leave alone, and the next tick is soon enough. Measured while building this: the `hold` variant
**deadlocks the sweep** behind whoever holds the checkout, which for a resume is a bundle fetch.
A background tick that can be blocked for the length of an unrelated operation is a worse bug than
the one being fixed.

`cleanup` waits, because there is a person to be told what happened, and waiting produces the
right sentence for free: the rebuild finishes, `cleanup` reloads the run inside the hold, finds it
no longer terminal, and says *still active; cancel it first* — which is what it always said and is
now true at the moment it is said.

### 5. `cleanup` takes the hold *before* the load

Taking a lock is an `await`. Loading first and holding second would put the terminal check back on
the far side of one — reintroducing, in the act of fixing a race, the exact shape two handoffs had
already cleared this function of. `Failed` is terminal *and* resumable, so `offload rm` and
`offload resume` on one run are two commands one keystroke apart.

## Consequences

* **A rebuild racing a teardown now waits and then rebuilds correctly**, rather than adopting a
  directory that is being deleted. Nothing is lost that was not already the losing caller's to
  lose: the removal passed its own guards legitimately at the moment it started.
* **`WorkspaceManager` is no longer a plain path.** It carries an `Arc<Mutex<HashMap<..>>>` of
  per-run mutexes, shared by every clone — a lock a clone does not share is not a lock, and the
  type is `Clone`. Entries are never reaped: proving nobody is about to take one is the race this
  exists to close, and the cost is a few dozen bytes per run this daemon has ever built a checkout
  for.
* **A poisoned map is stepped over, not propagated.** A panic inside a *different* run's guard
  construction holds nothing of ours, and refusing every checkout for ever afterwards would turn
  one bad run into a node that can neither start nor reclaim anything.
* **Wide, shallow edit.** Every caller of the four methods changed shape, tests included. No wire
  message, no schema migration, no gossiped field, no new run state.
* **Checked, both doors, both ways.** `a_rebuild_never_adopts_a_checkout_a_teardown_is_removing`
  in `offload-workspace` walks the window and fails at delay 0 with the exclusion removed;
  `the_sweep_skips_a_checkout_that_is_being_built` in `offload-node` is deterministic and fails
  with the same one-line revert. Both were watched red before green.

## What this deliberately leaves

* **In-process only.** Two daemons over one state directory would need a file lock. That is not
  this, and it is not a gap: a node owns its state directory, and the daemon that does not own it
  has bigger problems than a worktree.
* **The contents of a checkout are not guarded, on purpose.** `checkpoint::capture` and
  `checkpoint::restore` take a `Workspace`, which only `prepare` and `adopt` hand out, and the
  agent writing into its own worktree is the run's business. What is serialised is the directory
  coming into and going out of existence.
* **`git worktree add`'s own internal race is still git's**, as ADR-0051 left it. What has changed
  since is only that this daemon no longer runs two of anything at one path.
* **The three exclusions stay three.** ADR-0050 is one placement round per run per node, ADR-0051
  is one agent per run per node, and this is one hand on a checkout. They are about the same
  directory and they are not the same invariant, and a single lock naming all three is how the
  next session gets a surprise — the key here is a *checkout*, which outlives the agent, outlives
  the round, and belongs to the disk rather than to the run.
