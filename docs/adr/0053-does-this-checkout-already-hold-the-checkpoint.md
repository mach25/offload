# ADR-0053: A bundle is applied unless the checkout already holds it — asked of git, not inferred from a branch name

**Status:** accepted · 2026-09-05 · amends nothing · supersedes nothing

## Context

A checkpoint carries the agent's work in two blobs: a **bundle** of the commits it made on the
run branch, and a **patch** of what it had not committed. `Supervisor::restore_workspace` decides
which of them to apply when a run starts somewhere.

Until now the bundle was applied only when the run branch was **absent** from this node's mirror:

```rust
let has_branch = self.workspaces.has_branch(source, &branch).await;
let start_ref = if has_branch { branch.clone() } else { checkpoint.base_commit.clone() };
let bundle = match (has_branch, checkpoint.bundle) {
    (false, Some(hash)) => Some(self.store.get_blob(hash)?),
    _ => None,
};
```

The comment beside it said the commits "are already here", and named the hazard it was avoiding:
`restore` finishes with `git reset --hard`, so applying a bundle over a branch that has moved
*past* the capture walks the run backwards to where the checkpoint found it. That hazard is real.
A checkpoint is taken at a turn boundary and the agent keeps working, so a branch ahead of the
newest capture is the ordinary state of a live run, not an edge.

**But `has_branch` answers a different question.** A branch of that name is in the mirror whenever
this node has *ever* run a leg of this run — and nothing keeps it current. So a branch that is
merely **older** than the checkpoint suppressed the bundle just as thoroughly as one that was
newer, and the run carried on from a checkout missing every commit made anywhere else.

### Measured

Three legs on two daemons, no crash, no failure reported anywhere:

| leg | node | result |
| --- | --- | --- |
| 1 | alpha | turns 1–4, four commits, `offload checkpoint` releases it |
| 2 | bravo | `rebuilt from bundle and patch` — alpha's four arrive, four more committed |
| 3 | alpha | **`re-checked out, patch reapplied`** — bravo's four are gone |

Alpha's returning checkout held four of the eight files. The bundle blob had been replicated to
alpha, was named by the checkpoint, and was never opened. `offload logs` said `re-checked out,
patch reapplied`, which is true and reads as a successful resume. An earlier pass of the same
staging, with six turns a leg, had alpha's six commits and the returning checkout sharing
**only the base commit** — `git rev-list` on both branches intersected in exactly one hash.

The second way in needs no migration at all: any leg that fails between `prepare` — which creates
the branch at the base commit — and `restore` leaves the branch behind at base, and every
subsequent resume on that node then skips the bundle for ever. Measured while walking session
fifty-six's other fix, which is how this was found.

The test that should have caught it, `a_worktree_from_an_earlier_leg_is_not_mistaken_for_the_
current_one`, stages exactly this — "a run that comes *back*" — and sets `bundle: None` on the
returning checkpoint, putting the far leg's work in an uncommitted file. It walked the patch and
never the bundle.

## Decision

1. **`restore_workspace` stops deciding.** The bundle is read from the store whenever the
   checkpoint names one, and handed to `restore` unconditionally.
2. **`restore` decides, by asking git.** It fetches the bundle into `refs/offload/restored` as
   before, and then asks `git merge-base --is-ancestor refs/offload/restored HEAD`. If this
   checkout already contains the bundle's tip, the `reset --hard` does not happen. That is the
   question the old guard was standing in for, and only git can answer it.
3. **The answer is a reason, not a bool.** `BundleOutcome` is `Absent | Applied { left_behind } |
   AlreadyHere`. The caller has to be able to tell *left alone on purpose* from *there was
   nothing to apply*, because they produce different sentences in the run's own log, and
   "why is my work not here" is the question this whole path exists to answer.
4. **A reset that moves past commits this checkout has and the bundle does not names the hash it
   leaves.** They stay reachable through the reflog; `left_behind` is `Some(short)` only when
   `HEAD` is not an ancestor of the restored tip, so the ordinary older-branch case stays quiet.
5. **`has_branch` keeps its other job.** It still chooses the start ref: an existing branch is
   checked out rather than the base commit, because it may hold a leg that is ahead. Only the
   bundle decision moved.

### Why not the alternatives

* **Record the branch head in the `Checkpoint` and compare.** It is a new field on a gossiped,
  wire-visible type — an ADR-0005 ownership question and a wire bump — to obtain a fact git
  already holds and can be asked for in one subprocess. It also has to be right at capture time,
  which is one more thing to get wrong.
* **Always `reset --hard` onto the bundle.** Simpler, and it trades a silent loss of somebody
  else's commits for a silent loss of our own. A live run's branch is normally ahead of its
  newest capture, so this would fire constantly.
* **Merge rather than reset.** A merge can conflict, and there is nobody to resolve it: this runs
  on a tick or behind `offload resume`, with no operator in the loop. The design's answer to two
  divergent legs is that the epoch decides which one is the run (ADR-0051), not that they are
  reconciled.
* **Fix it upstream by pruning stale branches.** It attacks one of the two ways in — the failed
  leg — and not the migration, where the branch is not stale but simply older. It also throws away
  the very commits that make an ahead branch worth keeping.

## Consequences

* A run that migrates away, commits, and comes back keeps what it did while it was away. This is
  the headline behaviour of the project and it did not work.
* One extra `git merge-base` per restore that has a bundle, and a second one only when the reset
  is going to happen. Both are local and sub-millisecond.
* The run's log gains two sentences — `the commits here already held this checkpoint` and the
  `left_behind` clause — and both are new things an operator can be told rather than new states.
* **Residual: divergence is reported, not resolved** — *half closed, session sixty-two.* When a
  checkout has commits the checkpoint does not, the reset still happens: the epoch decides which
  leg is the run, and a node whose branch diverged is a node whose leg lost. What changed is what
  becomes of the commits. "Named in the log and kept by the reflog" was two weak halves — an
  unreachable commit's reflog entry expires (30 days by default, sooner if an automatic `gc` runs)
  and a hash in a log line needs somebody to know to look and to still have the log. They now get a
  **ref**, `refs/offload/left-behind/<run>/<short>`, which is the same answer `supersede` gives a
  checkout it moves aside rather than deletes, in this project's own namespace and safe from the
  one thing that deletes refs here (`ensure_mirror` is deliberately not a `--mirror`, so
  `fetch --prune` touches only `refs/remotes/origin/*`). Writing it is best effort and the run's
  log says which happened, because "only the reflog has it" is worth stating rather than implying.
  **Closed, session sixty-four (ADR-0055)** — and the half that asked to tell the *fleet* was
  answered by not doing it. A rescue is a fact about one disk with nothing to merge against, so
  both of these are per-node audit rows now (`AuditEvent::Rescued`), the way `Reclaimed` already
  was; `RunProgress` could not have carried it anyway, since its one writing leg is the leg that
  *kept* the run. And the removal half turned out to be upstream of any sweeper: `supersede`
  deleted the rescued copy's `.git` file, so nothing could ever tell a redundant copy from the one
  holding somebody's only mid-turn edit — measured, `git status` in a rescued copy answers `fatal:
  not a git repository`. It asks `holds_uncommitted` **before** the rename now and removes a clean
  checkout instead, whose commits are on the run branch. The ref itself is still written
  unconditionally and still kept for ever, which is right: those commits exist nowhere else.
* ~~**Residual: the `AlreadyHere` arm is covered by a test rather than a walk.**~~ **Closed**, one
  session later, and the staging is now in `docs/DEMO.md`. A same-node resume normally takes
  `adopted in place` and never reaches `restore`, which is what made this look hard: `adopt` reads
  the worktree's turn marker, and after a `kill -9` the marker is at the last *capture*, so it
  equals `checkpoint.turns` and counts as current. The route that does reach `restore` is the one
  this ADR's own context names — **the branch outliving the worktree**. `every_turns = 5`, kill
  between captures (marker at turn 5, branch at `L1 t8`), remove the checkout the way the product
  does (`git worktree prune`, which `supersede` and `remove` both run and a bare `rm -rf` does
  not), restart, `offload resume`. Measured against a build with the `--is-ancestor` check forced
  false, on the same state directory:

  | | guard removed | with the guard |
  | --- | --- | --- |
  | the log says | `rebuilt from bundle and patch; this branch was at cef5d49, which this checkpoint does not contain` | `the commits here already held this checkpoint; patch reapplied` |
  | files kept | `work-L1-1..5` — **turns 6, 7 and 8 gone** | `work-L1-1..8` |

  Decision 4 also earned itself: the broken build named `cef5d49`, the tip it was about to leave,
  so even there the hash is on the screen.
