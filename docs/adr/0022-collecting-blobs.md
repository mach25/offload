# ADR-0022: The blob collector runs by itself, because the person it was waiting for is not coming

**Status:** accepted · 2026-08-26 · third application of the premise ADR-0020 §6 broke ·
supersedes nothing

## Context

`blobs::collect_garbage` has existed since phase 2, is careful, is covered by two property tests
— one of which says in as many words that it is checking "the property a person actually relies
on when they put this on a timer" — and is called by **nothing**. Its own doc comment says why:

> Deliberately conservative and **deliberately manual**. Reclaiming a checkpoint that a run still
> needs turns a resumable run into a lost one…

Conservative is right and has already been earned the hard way: the version this replaced matched
blob hashes as hex against JSON that spells them as arrays of integers, so it found *no*
references and deleted the checkpoints of live runs. Manual is the part that has stopped being
true, for the third time in three ADRs. `Supervisor::cleanup` was never automatic because
somebody would read the worktree; ADR-0020 §6 found the path where nobody does.
`Store::delete_run` was never called because somebody had to decide what happened to news still
owed; ADR-0021 decided it. This is the same sentence a third time: **manual means somebody runs
it**, and nobody logs into the machine that hosts the runs.

What that costs, measured rather than reasoned about. One run, six turn boundaries, a fake agent
whose transcript grows the way a conversation does:

```
blob files: 6        851,067 bytes
sizes:      40,527  81,054  121,581  162,108  202,635  243,162
referenced: 1        243,162 bytes   (the checkpoint at turn 6)
```

**71% of it is unreachable, from one run.** `Run::record_checkpoint` *replaces* the checkpoint, so
every turn boundary after the first orphans the previous transcript, bundle and patch — and a
transcript is the whole conversation so far, so the garbage is quadratic in the conversation's
size rather than linear. A thirty-turn run against a real agent leaves tens of megabytes that
nothing will ever look at again, on every node that held or replicated it.

This is not new and it is not caused by triggers. What ADR-0020 changed is that runs now arrive
**by themselves, for ever**, on a machine nobody logs into — so the leak stopped being bounded by
how often a person starts work.

## Decision

### 1. The daemon collects, on a tick of its own

A loop beside `tend_own_runs` and `deliver_notifications`, for their reason: this is about *this
node's own disk*, so putting it in the gossip tick would mean it silently does not run on a fleet
of one — which is precisely the setup where nobody is going to notice.

Every **fifteen minutes**. Nothing here is racing anything; the cost of a slow pass is disk that
is already spent, and the pass reads every run row, which is not something to do beside a
heartbeat.

### 2. An hour of grace, and the grace is the whole safety argument

`older_than_ms` is one hour. A blob is written *before* the row that references it — `capture`
puts the transcript, bundle and patch and only then records the checkpoint — so between those two
writes the blob is unreferenced and collectable, and the same window exists on the receiving side
of a replication (ADR-0016). The grace has to be generously longer than any such window. An hour
is roughly twelve times the gossip tail and some thousands of times the actual gap, because the
cost of waiting is disk that is already lost and the cost of being early is a run that cannot
resume.

### 3. A superseded replica is not worth protecting, and that is why this is safe

The hazard worth stating, because it looks fatal and is not. This node holds a **replica** of a
peer's checkpoint at turn 10. The holder checkpoints at turn 20 and gossips the record; our copy
of that record now names the turn-20 blobs, so our turn-10 replica is unreferenced and — an hour
later — collected. If the holder then dies, has the fleet lost the run?

No, and the reason is worth remembering rather than trusting: **recovery fetches the blobs the
*record* names.** A new holder resumes from the checkpoint in the run's record, which is turn 20.
The turn-10 replica was already unreachable the moment the record moved past it; keeping it would
protect nothing, because nothing knows how to ask for it. The blob that matters is the one the
record names, and that one is referenced and never collected — which is exactly the property the
existing tests check.

### 4. What is *not* built

**No size or age bound on referenced blobs.** A cap would mean deleting a checkpoint a live run
needs when the number was reached, which is the one thing this must never do. Disk pressure is a
real problem and this is not the mechanism for it; the answer there is fewer runs or a bigger
disk, and the honest first step is that `offload status` can now show a number that stops
growing.

**No collection on demand.** There is no `offload gc`, because the failure it would exist for —
"the disk is full and I want the space back now" — is answered by the tick within fifteen
minutes, and a command that deletes data is surface worth not having.

**Nothing changes about how conservative the pass is.** It still refuses to run at all if a
single run row will not decode ("a run whose references are *unknown* is not a run with none"),
which on a timer means the whole pass fails loudly and repeatedly rather than quietly collecting
the wrong thing.

### 5. A checkpoint stops being one when the run stopped by decision

**Amendment, session twenty-six, found by asking what the tick left behind.** §3's argument —
recovery fetches the blobs the *record* names, so anything the record has moved past is
unreachable — has a second half that was never drawn. It applies to the checkpoint the record
*still* names, the moment nothing can start an agent from it.

`Supervisor::resume` refuses `Completed` and `Cancelled` **by name**: they are decisions, and
re-opening one would restart work somebody deliberately stopped. Nothing else fetches a
checkpoint at all. So past one of those two states the blobs the record names are reachable by no
path there is — and they are the *largest* checkpoint the run ever took, because
`record_checkpoint` replaces and a transcript is the whole conversation so far. This tick
collected the five cheap ones and kept the expensive one.

Measured on the same fake agent as the numbers above, six turn boundaries: **one blob kept, at
472 KB, per completed run**, on the home node, on the leg that ran it, and on every peer that
took a replica. Session twenty-five's walk saw it and wrote it down as success — "the store
settling at exactly one blob per completed run". The right number is zero.

`Failed` is the exception, and it is why the rule is not `is_terminal`: it is the one terminal
state that reopens (ADR-0013), so its checkpoint is the one whose loss is unrecoverable. The
walk confirmed both directions rather than just the cheap one — a failed run's checkpoint survived
three passes and auto-resume picked it up and carried on at turn 5.

`Run::resumable_checkpoint` is the predicate and `referenced_blobs` is its one collector-side
caller. It is written over `RunState::may_resume_later`, whose arms are **written out** rather
than derived from `is_terminal() && !is_failed()`: a new terminal state has to make this decision
rather than inherit it, because the two wrong answers are not symmetrical — answering `false` for
something resumable deletes the only copy of a conversation — and the compiler is the only thing
that will ask.

The same predicate corrected two *reports* that had been asking `is_terminal` and were therefore
silent about the run that reopens. `offload ps` suppressed the `here only` warning for every
terminal run, so the one row it was written for — a failed run whose checkpoint exists on one
machine — never showed it; `offload explain` printed a bare turn number in the same case. And
`Checkpoint::is_durable` takes an `Option<NodeId>` now, because extending those two to a run with
**no holder** would otherwise have introduced a new wrong answer: there is nothing to discount,
and `replicas` never contains the node that took the checkpoint, so the question is simply whether
anybody else has it. Both callers had invented a holder — one substituted the local node, which
answers about a peer's copy on a peer, and one read no holder as no copy, reporting three replicas
as `on one machine only`.

## Consequences

Good: the disk a node uses stops being a function of how long the daemon has been up. ADR-0021
bounded the *rows* a rule leaves behind and claimed more than it delivered; this is the other and
much larger half — 600 KB per six-turn run against the ~1.5 KB per record the other one was
about.

Bad:

* **The daemon deletes files by itself now.** Bounded by a pass that has two property tests and
  an hour of grace, and by nothing else. The blast radius of a bug here is a run that cannot
  resume, which is the failure this project spends the most effort avoiding — so a change to
  `referenced_blobs` or to `Checkpoint::blobs` is now a change to a deletion rule, and should be
  read as one.
* **A pass reads every run row, every fifteen minutes.** Fine at the scale this runs at and worth
  knowing before somebody keeps a year of records on one node.
* **An unreferenced blob still survives an hour.** A rule firing every three seconds against a
  real agent can therefore hold an hour's garbage at any moment. Bounded, which is the point, but
  bounded at a number chosen for the safety argument rather than for the disk.
* **A completed run's record names blobs that are gone** (§5). Honest about what the last capture
  *was* and no longer an offer to restore from it, which is why `offload explain` says `last
  captured at turn 6` rather than `checkpoint`. Clearing the field instead was considered and
  rejected: the checkpoint travels with the record, so a stale peer copy winning a merge would
  reinstate the reference anyway — the dangling state is reachable either way, and the version
  that keeps the field at least records the history.
