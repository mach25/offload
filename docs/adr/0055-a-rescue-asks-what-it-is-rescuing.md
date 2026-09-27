# ADR-0055: A rescue asks what it is rescuing

**Status:** accepted · 2026-09-08 · amends ADR-0053 (its remaining residual) · amends ADR-0023
(a third `Reclamation` door)

## Context

Two paths in this design **set something aside** rather than let it go, both on the code that
rebuilds a checkout from a checkpoint:

* `CheckoutGuard::supersede` renames a checkout from an earlier leg instead of removing it,
  because it may hold the only copy of a mid-turn edit (ADR-0003's valuable part).
* `checkpoint::keep_reachable` writes `refs/offload/left-behind/<run>/<short>` for commits a
  `reset --hard` is about to move the branch off (ADR-0053, session sixty-two).

ADR-0053's residual said what was still wrong with both in one sentence: *nothing tells the fleet,
and nothing ever removes either.* This ADR answers it, and the answer to the first half is that
telling the *fleet* was the wrong target.

### The rescue destroys the evidence for its own necessity

`supersede` renames the checkout and then deletes the rescued copy's `.git` file — correctly, since
it would point at bookkeeping the following `worktree prune` removes. What nobody noticed is what
that costs. **Measured**: `git status` inside a rescued copy answers `fatal: not a git repository`.

So `holds_uncommitted` — the exact question `reclaim_occurrence` uses to tell litter from
treasure, and the one whose wrong answer deletes somebody's work — is available *before* the
rename and unavailable for ever afterwards. Renaming unconditionally therefore did not keep the
decision open for later; it closed it, in the direction of keeping everything.

The consequence, measured at the crate level: three legs of one run left **three full copies of
the tree** and three refs, none of them distinguishable from the copy that mattered, and nothing
in the product ever removed one. `free_superseded_path` counts to 1,000.

And a clean rescue is redundant by *measurement*, not by judgement: committed work is on the run
branch in the mirror, which `WorkspaceManager::remove` keeps on purpose and says so.

### The other half was never the fleet's business

A rescue is a fact about **one disk**. There is no second opinion anywhere to merge with it, which
is the argument `AuditEvent::Reclaimed` already made for the checkout sweep (ADR-0023, and the
pitfall entry beside it): sometimes the answer to "who owns this gossiped field" is that it should
not be a field. Both rescues were a `tracing::warn!` on an unattended machine plus a sentence in
the **run's** log — and a run's log is served by its *holder*, while the leg doing the rescuing is
by construction the leg that lost the run. Once the run is deleted, nobody can read it at all.

### Measured

One daemon, both arms on a byte-identical state directory: a run committing each turn, checkpointed
at turn 5 and left `pending`, its turn marker removed so `adopt` answers `Superseded { at_turn:
None }` — the *run in flight across an upgrade* case the marker exists for — then `offload resume`.

| | before (`&& false`) | after |
| --- | --- | --- |
| `offload logs` workspace line | `…the checkout here held turn unknown and uncommitted work, so it is kept at …superseded` | `…and nothing uncommitted, so it was removed — its commits are on the run branch` |
| `<state>/worktrees/` | `<run>`, **`<run>.superseded`**, `<run>.turn` | `<run>`, `<run>.turn` |
| `offload audit <run>` | *(the row is new either way)* `kept an earlier leg's checkout here…` | `reclaimed this run's checkout here: an earlier leg's, holding nothing uncommitted…` |
| the resumed run | turn 5 → 8, `completed`, five commits kept | identical |

Every one of the six files in the copy the control kept for ever was checked against the run
branch, and every one of them was on it. The control's sentence is also **false about a clean
checkout** — it claims uncommitted work that is not there — which is what an unconditional rescue
leaves a report no way to avoid saying.

## Decision

1. **`supersede` asks before it moves.** `WorkspaceManager::holds_uncommitted` is the question,
   asked while this is still a worktree and answered by git. A checkout holding nothing goes
   through the ordinary teardown — `remove`, so `git worktree prune` runs and the run's branch is
   freed for the rebuild, which a bare `rm -rf` does not do.
2. **Unknown is not nothing.** `holds_uncommitted` answers `None` for a worktree it could not
   read, and that **keeps** the copy. Same direction its other caller takes, and the only safe one
   at a door whose wrong answer deletes somebody's only copy of a file.
3. **The answer is a reason, not a path.** `Rescued::{MovedAside(PathBuf), Redundant}`, for
   `BundleOutcome`'s reason one layer down: the caller has two different sentences to write and
   *removed* is now the ordinary case, so a line that only ever appears for the rescue teaches
   nobody to expect either.
4. **A rescue is a per-node audit row**, not a gossiped field and not only a `tracing` line.
   `AuditEvent::Rescued { run, what: Rescue }`, with `Rescue::{Checkout, Commits { named }}` — and
   `named: false` for the case worth stating rather than implying, where the ref could not be
   written and the expiring reflog really is all that holds the commits.
5. **A redundant removal is a `Reclaimed` row with a third door**, `Reclamation::Redundant`, since
   the disk changed and what a reader wants is *which decision* took it (ADR-0023's own reasoning
   about `why`). It sits beside `MovedOn` and `NobodyWaiting`, and it is the one that is a
   measurement rather than a judgement about who is watching.
6. **The run id is the whole address**, which is why neither row carries a path or a hash. Both
   names are derived — `<state>/worktrees/<run>.superseded*` and
   `refs/offload/left-behind/<run>/*` — so the row plus the two conventions is enough to find what
   was kept. The specifics stay in the run's own log, where there is somebody following it.

### Why not the alternatives

* **A retention policy, or a sweep with a grace period.** It needs an age at which somebody's
  only copy of a mid-turn edit stops mattering, and there is no such age. It also adds a tick, a
  constant and a config knob to remove things that mostly should never have been created — the
  `collect_garbage` shape, aimed at the symptom. Asking at the source needs none of them.
* **Keep `.git` so the copy stays askable later.** It would point at a worktree registration
  `prune` is about to remove, which is why it goes. Re-pointing it is a second bookkeeping
  relationship to maintain for a directory whose whole purpose is to be a plain pile of files.
* **Record what was uncommitted, in the row, and keep renaming.** Strictly more information, and
  it answers the wrong question: an operator with a list of six filenames still has a full copy of
  a tree they cannot act on, and `AuditEvent` stops being `Copy` to carry it. The useful thing is
  for the redundant copy not to exist.
* **Tell the fleet**, as ADR-0053's residual literally asked. There is nothing to merge and no
  other node with an opinion, and `RunProgress` is a fleet-agreed record whose one writing leg is
  the leg that *kept* the run — so the write would be correctly dropped on the very node doing the
  rescuing. Session seventeen's bug, and the reason `Reclaimed` exists.
* **Remove a superseded checkout unconditionally.** The case this whole path exists for: a
  checkout mid-turn when the run was taken away holds edits no checkpoint captured.

## Consequences

* A node stops accumulating full copies of trees it can prove it does not need. The rescues that
  remain are the ones that rescued something, which is what makes the warning worth reading.
* Two new sentences in the run's log, and the *ordinary* resume now says which of the two happened
  rather than being silent about a directory it removed.
* `offload audit <run>` answers "what has this machine set aside, and what did it throw away" from
  one log, per node, for a run whose record may be long gone.
* **Residual: the rescues that remain still have no retention, and that is deliberate.** A copy
  holding uncommitted work and a named left-behind commit are both kept for ever, because nothing
  else has them and no age makes them worthless. What has changed is that there are far fewer of
  them and each one is now recorded where it can be found. If the count ever becomes a problem the
  lever is a report — `offload audit` already lists them — not a sweeper.
* **Residual: `refs/offload/left-behind/<run>/…` is still written unconditionally**, since those
  commits genuinely exist nowhere else; only the *checkout* half had a redundant case to detect.
  A run branch that is deleted would strand them, and nothing deletes a run branch today.
* Both rescues can happen to one run in one resume — a redundant checkout removed *and* commits
  named — and that combination is what `offload audit` shows an operator. It was correct
  unstaged, and is staged now; the first fixture written for this ADR set `bundle: None` and could
  not reach it, which is the trap this file's own pitfall entry names.
* **Residual: nothing checks a new arm of `Reclamation`, `Rescue` or `Attempt`.** The audit
  round-trip's list is hand-written, and `Rescued` is how that was found — a variant added with no
  line there is a variant nothing has ever encoded. `the_round_trip_covers_every_kind_there_is`
  now counts `AuditEvent`'s own variants against it, from the enum's source, for `no_clock.rs`'s
  reason. It does not reach the nested payload sets, and that is said in the test rather than
  implied.
