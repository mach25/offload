# ADR-0023: A node reclaims the checkout of a run that is not its own any more

**Status:** accepted · 2026-08-26 · fourth application of the premise ADR-0020 §6 broke ·
supersedes nothing

## Context

`WorkspaceManager::remove` has exactly one production caller — `Supervisor::cleanup`, which is
manual and terminal-only. CLAUDE.md has stated the consequence since session sixteen, as a note
about `adopt` being wrong to infer "current" from "exists":

> nothing removes a worktree when a run leaves

That sentence is true and it has never been read as a *leak*. It is one, and ADR-0020 turned it
into an unbounded one on a machine nobody logs into. Two checkouts nothing will ever remove:

* **One a run left.** A run migrates, or is reassigned after a hold-down, and the machine it left
  keeps a full checkout of the repository. Its work went with it: the checkpoint travelled, and
  committed work is on the run branch in the mirror, which `remove` says in as many words.
* **One an occurrence was placed on.** ADR-0021 reclaims a rule's occurrence checkout "only where
  the checkout is" — right, and on a **peer** there is no rule, no firing, and nothing that will
  ever ask. So a rule whose occurrences the fleet places elsewhere leaves one checkout per firing
  on a machine that has no idea a rule exists. That is the same runaway the records had, one
  directory bigger: a checkout is megabytes where a record is kilobytes. Measured on two daemons,
  a rule ticking every five seconds and alpha configured not to accept work: **62 firings, 62
  worktrees on beta, 145 MB** — five minutes, from a repository of 3.9 MB.

  **This ADR reaches the first and not the second**, which is a change from what it set out to
  do and is worth stating where the claim was. §2 is why: the guard that says a run has moved on
  is the leg that last produced its position, and for an occurrence that *finished on the peer*
  that leg is the peer. Its checkout is indistinguishable from an ordinary run somebody is about
  to read the output of, because nothing travelling says otherwise. Same missing fact as the
  records a peer keeps for ever, and the same open question.

This is the fourth statement of one premise. `cleanup` is never automatic because somebody will
read the worktree. `delete_run` was never called because somebody had to decide about news still
owed. `collect_garbage` was manual because somebody would run it. Each names a person, and each
was written before there was a path with no person on it.

## Decision

### 1. A node reclaims a checkout for a run it is not running, on the housekeeping tick

The tick ADR-0022 built, because it is the same question about the same disk and because the node
that most needs this — a peer hosting somebody else's occurrences — has no firing of its own to
hang it on. Checkouts go first in the pass: one is megabytes where a blob is kilobytes.

### 2. Five guards, and each is a rule from somewhere else

* **We know the run.** A checkout whose run this node has no record of is left alone. Unknown is
  not none, and for a directory the safe reading of not knowing is to keep it.
* **We are not its holder.** A run that is ours is one we may be about to resume, and its checkout
  is what makes that cheap.
* **No agent of ours is live on it.** The guard `holder` cannot supply, and the reason it is
  separate: a leg that has just been **superseded** is no longer the holder while its process is
  still being taken down — and that process is writing into this very directory.
* **Another leg produced the run's latest position.** The guard that says the run has *moved on*,
  and the one the first draft of this got wrong badly enough to be worth recording. "Not the
  holder" seems to say it and does not: a **finished run has no holder at all**, so that test is
  true of every machine that ever finished a run, and a sweep built on it would delete the
  checkout `cleanup` is manual for. `Supervisor::describes` answers the question correctly and
  only for *this incarnation* — it reads the in-memory `live` map, so after a restart it says no
  about every run on the disk, which is the same mistake one door along. `RunProgress::by` is the
  durable form: the leg that last wrote a run's position is by construction the leg that ran it,
  which is what session twenty-three's `offload logs` fallback already rests on. An **unstamped**
  record is unknown rather than somebody else, and keeps the checkout.
* **Nothing uncommitted in it.** ADR-0020 §6's bound verbatim and for its reason, asked of git
  rather than of the worktree summary beside the run, because that string is one turn boundary
  stale by construction and this is the question whose wrong answer deletes somebody's work.

No grace period, unlike ADR-0022's blobs, and the difference is worth naming: a blob needs one
because it is written *before* the row that references it, so there is a window in which the
truth is not yet on disk. These guards are about who holds the run and what is in the directory —
facts, not windows.

### 3. Nothing is written down about it

`cleanup` records `workspace: "removed"` beside the run. This must not, and the difference is
`describes`: a leg that has lost a run may not say what its worktree holds, because those numbers
gossip and `accept_progress` breaks a tie on the author's clock — so a copy matching the holder's
counters with a later stamp wins the whole fleet. That is session seventeen's `offload rm` bug,
and it is worse here for ADR-0021's reason: there was a person typing `rm`, and this runs by
itself every fifteen minutes.

### 4. What is *not* built

**No reclaim of a checkout for a run we hold but are not running** — a dormant run recovered
after a restart is exactly the case `adopt` exists for, and taking its checkout away turns a
cheap resume into a cold clone.

**No reclaim of a `supersede`d checkout.** `adopt` moves an outdated checkout aside rather than
deleting it, "because it may hold the only copy of a mid-turn edit", and those moved-aside
directories still accumulate. Left alone deliberately: the premise there names *work*, not a
person, so ADR-0020's argument does not reach it, and what would is a rule about when a mid-turn
edit stops being worth keeping. That is a different question and it should be asked on its own.

### 5. What the walk covered, and what it did not

Said plainly, because the headline case is the one it missed. Two daemons, a rule ticking every
five seconds, alpha configured not to accept work: **62 firings, 62 worktrees on beta, 145 MB in
five minutes** — the leak, measured. With the sweep running, a run that **finished on alpha**
kept its checkout across two passes with zero reclaims, which is the regression §2 is about and
the one worth watching live.

The **migration** case — a run that leaves — is covered by a test over real worktrees and *not*
by the walk. The attempt failed for a reason worth writing down rather than retrying blindly:
the daemons were started without `CLAUDE_CONFIG_DIR`, so every checkpoint failed with "no
transcript found", so the drain had nothing it could hand over and the agent simply ran to
completion where it was. A migration walk needs that variable set on **both** nodes, which is
the same demo-friction note ADR-0022's walk produced, met from the other side.

## Consequences

Good: a migration stops costing a permanent checkout on every node a run ever visited, which is
the leak CLAUDE.md has described since session sixteen without anybody reading it as one.

Bad:

* **A run that comes back to a node it left now finds nothing to adopt.** `Adoption::Absent`
  rather than `Superseded`, so it restores from the checkpoint — which is what it did before,
  because a superseded checkout was never adopted either. What is actually lost is the *chance*
  that the checkout was still current, and the holder guard is what keeps that case.
* **The daemon deletes directories by itself now.** The blast radius is somebody's uncommitted
  work, guarded by a question asked of git and by a real-worktree test for each of the five
  guards. A change to `holds_uncommitted` is now a change to a deletion rule.
* ~~**An occurrence that finished on a peer still leaves its checkout there.**~~ **Closed by
  ADR-0024, the same day**, which carried exactly the fact this consequence said was missing —
  and it was right that inventing a second way to carry it would be two mechanisms for one
  sentence. Verified in session twenty-six rather than inferred, because the sweep's own doc
  comment and two doc files went on repeating this sentence after it stopped being true: a rule on
  alpha, alpha refusing to host, every occurrence placed on beta and finished there, and beta's
  worktree count oscillates between 0 and 5 over five minutes of firing every three seconds where
  it used to grow one per firing.
* **A checkout for a run this node has no record of is kept for ever.** The conservative branch,
  and it is reachable: prune a record (ADR-0021) whose checkout could not be reclaimed because it
  held uncommitted work, and nothing will look at that directory again. Keeping it is the right
  answer — it is uncommitted work — but nothing now says it is there.
