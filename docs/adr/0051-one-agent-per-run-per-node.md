# ADR-0051: One agent per run, per node — decided where the agent is claimed

**Status:** accepted · 2026-08-30 · extends ADR-0050 · closes the open entry in
`docs/pitfalls/fencing-and-epochs.md`

## Context

Everything that stops two agents working on one repository is written in terms of the **epoch**:
a grant spends a token whether or not it is confirmed, two live grants at one epoch are ordered by
lowest holder id, a holder whose record names somebody else at its own epoch stops its agent, and
— since ADR-0050 — one node runs one placement round per run.

None of that reaches two legs **on one node holding the run at one epoch legitimately**. That is
not a fencing question at all. `Run::assign` was called once, by whoever started the first leg; a
second caller arriving afterwards finds a row that says *this node holds this run under a lease of
its own*, which is exactly what the running leg put there. `Supervisor::hold_here` waves it
through — correctly, and it is the branch that makes a start idempotent — and hands back the same
epoch it read.

What decided whether a second agent then started was `Supervisor::register`, and it decided
nothing: it `insert`ed into the live map unconditionally. `BTreeMap::insert` **replaces**. So the
running leg's entry — its cancel sender and its event stream — was dropped on the floor while the
leg itself carried on, and both legs went to `launch`, `drive`, and a workspace.

Three separate harms, in the order they land:

* Two `git worktree add` for one run, in one mirror, at one path.
* The displaced leg becomes **unstoppable**: `cancel_run` and `request_checkpoint` both find the
  live entry, and after the replacement the entry they find belongs to the other leg. The first
  agent's `cancel_rx` has no sender any more.
* Whichever leg loses the git race writes `Failed`, which is terminal, beats the live record
  everywhere, and fences out the healthy leg — so a run that nothing was wrong with ends `failed`
  with a raw git error, and `note_failure` makes it a candidate for auto-resume.

The callers were the only thing standing in front of this, and they do not all check:

* `start_held_runs` filters its snapshot on `live`, so it does check — **before** the loop, for a
  list it then works through one slow start at a time.
* `Supervisor::resume` checks, and then does two more things before registering.
* `Supervisor::take_run` does not check at all, and cannot: every capacity question it asks
  *excludes the run being granted* — `started_excluding(run.id)`, and a `queued` test that skips
  `held.id == run.id` — because the run is arriving right now and must not be counted against
  itself. A grant for a run this node is already running goes straight to `start_run`.
* `Supervisor::submit` does not check, and does not need to: its run id is new.

Four callers, two of which check, one of which cannot, and one that has no reason to — and a
fifth caller would be a fifth thing to remember.

### Measured

The symptom was recorded by session fifty-two as an observation with no mechanism: twice in eighty
daemon passes, alpha built two worktrees for one run 1ms apart off a single grant, one of them
failing with

```
cannot force update the branch 'offload/<run>' used by worktree at '<the same path>'
```

That sentence is the evidence, and it says more than it looks like. `WorkspaceManager::prepare`
runs `git worktree add --quiet -B <branch> <path> <commit>` and passes **no `--force`** — and
without `--force`, git's own `die_if_checked_out` fires first and says something else entirely.
Reproduced directly, outside this codebase, on git 2.55:

| what happened | what git says |
| --- | --- |
| the worktree already exists, added earlier | `'X' is already used by worktree at 'X'` |
| the directory was renamed away, not pruned | `'X' is already used by worktree at 'X'` |
| **two `worktree add` at once, same path** | **`cannot force update the branch 'X' used by worktree at 'X'`** |

200 forced pairs of concurrent `git worktree add --quiet -B rb <same path> <commit>`, counting
one message per failing side: 17 `cannot force update the branch`, 182 `is already used by
worktree`, one torn `worktrees/<name>/commondir`. `worktree add` is a read-decide-write too — `die_if_checked_out`
passes before the winner registers, and `create_branch` dies after — so the loser of a genuine
race is the only thing here that produces that wording.

So the session fifty-two logs are not evidence of a stale checkout or of anything upstream in
placement. They are two concurrent `prepare` calls for one run on one node, which is two `drive`
tasks, which is two legs past `register`.

The unit reproduction is the one that matters, because it is deterministic:
`a_second_start_for_a_run_already_going_here_starts_no_second_agent` holds a leg open inside
`ensure_blobs` — the only `await` in `start_run` — and calls `start_run` again with the row the
first leg wrote. Without the refusal the second call registers over the first and walks on into the
blob fetch; the test's timeout is the assertion, and it fails in five seconds with *the second
start went past the registration and into the blob fetch*.

## Decision

### 1. An agent for a run is claimed, and the claim is exclusive on the node

`Supervisor::register` returns `Result`. If an agent for the run is already going here it refuses;
otherwise it claims the slot and hands back the cancel channel. The check and the insert are one
`live` lock, so two legs cannot both read *nobody is going* and then both claim it — the same
read-decide-write shape that put `hold_here` inside `Store::update_run`, one map further out.

"Already going" is `agent_here`: an entry **with a cancel sender**, deliberately not the entry's
presence. A `LiveRun` outlives its leg, and a new leg of a run that came back is entitled to a
fresh stream, so the entry is still *replaced* where nothing is going.

### 2. In the writer, not in the callers

This is ADR-0050's third paragraph and `Supervisor::holds`'s: four call sites checking is four
things to remember, and the two that would have had to remember it are the two that are hardest to
see — `take_run`, whose whole capacity calculation is built on excluding this run, and
`start_held_runs`, whose check is a filter taken before a loop that awaits.

Nothing above `register` has to know, and a caller added later gets it without an audit.

### 3. The loser gets its own error, and does nothing with the run

`SubmitError::AlreadyGoing`, for the reason `Ended` and `LostTheRun` are their own variants: the
caller has to tell it from a start that failed. Nothing is wrong with the run — it is running — so
this is neither a failure to write down nor a refusal that will lift, and the leg that receives it
must not conclude, fail, or re-file the run. It must do nothing.

`start_run` claims **before** it records the acceptance or writes `preparing` on the run's summary,
so a leg that is not going to start leaves nothing behind on its way to finding out.

### 4. It says so out loud, because the two callers that met are still unnamed

The refusal logs at `warn` with both epochs — the leg that is going, and the one that asked. Equal
epochs are two legs born of one assignment; different epochs are a re-grant landing on a run this
node already runs. That is the question session fifty-two could not answer from two occurrences in
eighty passes, and the door now names itself the first time it opens on any machine.

The guard is not conditional on knowing the answer. It is the invariant, and it holds whichever
caller it turns out to be.

## Consequences

* **A grant for a run this node is already running is declined.** `take_run` propagates
  `AlreadyGoing`, so `Mesh::accept` answers `Declined`. That is the conservative answer and it
  converges: the arbiter re-offers past this node at a higher epoch, and this node's leg — whose
  record now names somebody else above it — stops itself. The alternative, answering `Ok` because
  the node does hold the run, ends in the same place by way of the same fence, having said nothing.
  Prefer a run stalled or moved over a run duplicated.
* **`start_held_runs` classifies it separately**, at `warn`, saying that an agent was already going
  rather than that a start failed. Its snapshot is filtered on `live`, so reaching that arm at all
  means two callers met — which is worth a line even though the run is fine.
* **The displaced-leg harm is gone with it.** A running leg keeps the registration it made, so it
  keeps the cancel sender `cancel_run` needs and the stream a follower is reading.
* No new state, no new field, nothing gossiped: the live map already knew, and nothing asked it.

## What this deliberately leaves

* **Which two callers met is still not known**, and this ADR does not claim to have found out. It
  closes the door for all of them and makes the next occurrence name itself. Two in eighty passes
  is not a rate anything here can chase by re-running the staging, and a guard whose correctness
  depends on having identified the caller would be the wrong guard.
* **`git worktree add`'s own race is untouched**, and it is not ours to fix. What changes is that
  this daemon no longer runs two of them for one run; two *different* runs never share a path or a
  branch. The wording table above stays in `docs/pitfalls/checkpoints-blobs-and-workspaces.md`
  because it is the cheapest way to tell a concurrent add from a leftover checkout, and the two
  need opposite fixes.
* **`git worktree remove --force` still spans an await with no exclusion**, exactly as ADR-0050
  left it. It wants a rendezvous keyed by checkout, and it is not this lock: this one names one
  agent per run, which is a different invariant that happens to be about the same directory.
  **Built in ADR-0052**, as a third primitive rather than an extension of this one, for exactly
  that reason — and it rests on this ADR's ordering: `register` claims the run before `launch`
  spawns `drive`, which is what lets the checkout lock be dropped before the agent starts.
