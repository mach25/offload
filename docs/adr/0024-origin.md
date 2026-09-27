# ADR-0024: A run says who started it, because two nodes cannot otherwise tell whether anybody will read it

**Status:** accepted · 2026-08-26 · answers open question 9 · amends ADR-0021 §1 and ADR-0023 §2

## Context

Three ADRs in a row have reclaimed something a run leaves behind, and each stopped at the same
edge. ADR-0021 prunes a rule's spent **records**; ADR-0022 collects unreferenced **blobs**;
ADR-0023 reclaims **checkouts** of runs that have moved on. Blobs were fleet-wide because a blob
is referenced by a record and every node has its own records. The other two are not, and the two
things they cannot reach were both measured this session, on a machine hosting nothing:

* **Records.** Alpha's rule settles at 101 records while **beta grows 81 → 181 in five minutes**.
* **Checkouts.** With occurrences placed on beta: **62 firings, 62 worktrees, 145 MB in five
  minutes**, from a repository of 3.9 MB.

One cause. `cleanup` is manual "because somebody will read the worktree", and ADR-0020 §6 found
the path where nobody will — but the *only* node that can tell those apart is the one holding the
rule, because a rule is node-local (ADR-0020 §2) and nothing about it travels. To every other
node an occurrence is an ordinary run: it has a prompt, it finished somewhere, and for all that
node knows a person is about to read the output.

So the reclaiming is right and its reach is wrong, and the fix is not a fourth sweep. It is one
fact that has never travelled.

## Decision

### 1. `Run::origin` — `Operator` or `Rule`

A field on `Run`, beside `home`. `home` says *which node* asked; this says *what kind of thing*
asked, and it is set once at creation by the node that created it.

**Who started it, not whether anybody is watching.** Those are different facts and this project
already has the second one: **attendance** is *observed, never declared* (ADR-0013) and
deliberately not gossiped, because a value from before the silence presented as current is a
confident wrong answer. Origin is the opposite in every way that matters — known at submission,
immutable for the run's life, and stated by the only node that could know. What the reclaiming
rules actually want is the inference "nobody will read this", and making that inference is this
ADR's decision rather than something the field asserts.

`Operator` is the default in every sense: the serde default, the value an older build's record
decodes to, and the answer whenever anything is unsure. It is the value that *keeps* things.

### 2. It is the cleanest kind of gossiped field: one that cannot disagree

ADR-0005 asks who owns a gossiped field and what arbitrates it. Here the answer is that the
question does not arise. `origin` is set when the run is created and no transition, no operator
command and no merge can change it — the same class as `id`, `home` and `created_at`, which ride
inside whichever record `merge_run` settles on and are identical in every copy. There is no merge
rule to get wrong, which is worth stating rather than leaving as an absence.

**No wire bump.** The rule in `offload-proto` is that a version costs a device falling out of the
fleet, so it is spent only when a node that ignores the new thing gets something *wrong*. A build
that drops this field reads `Operator`, keeps every record and every checkout, and behaves exactly
as it does today. Failing towards keeping is the whole design of the field.

### 3. This supersedes ADR-0021 §1's reasoning, and says why that was right at the time

ADR-0021 §1 put the rule id in a **node-local column** and argued: "a rule is node-local, so which
rule fired a run is a fact about one machine's own bookkeeping, and a gossiped field would need an
owner and a merge rule for something no peer can state."

Every clause of that is still true, and it is why `runs.rule` stays exactly where it is. What
travels is not the rule — a `RuleId` is one machine's name for one of its own things, useless to a
peer for the same reason an `Audience` names services and never route ids. What travels is the
much smaller fact that *a machine started this*, which any node can act on and none can disagree
about.

### 4. What each node may then do

**Records.** `spent_occurrences` stops being keyed on the rule and becomes a question about
machine-started runs, asked by every node on the housekeeping tick. Its clauses are unchanged and
they degrade correctly on a peer rather than being skipped there: a peer holds no events for a run
it never ran, so "nothing owed" and "nothing left to notice" are trivially satisfied, and the
quiet period and the completed-not-failed rule do the work. `runs.rule` survives for what it was
always for on the node that owns it — `except` at a firing, and the `KEPT` column.

**Checkouts.** ADR-0023's "another leg produced the run's latest position" is the durable way to
ask *has this run moved on*, and it is exactly wrong for an occurrence that finished on this
machine: that leg is us. Origin is the second door — a terminal machine-started run's checkout may
go whoever ran it, because the premise `cleanup` rests on has no person behind it. Both guards
keep every other clause, uncommitted work above all.

### 5. What is *not* decided here

**Nothing changes for an operator's run.** A record and a checkout on a peer for a run somebody
submitted are still kept for ever, and `ps --all` on any device still lists them. That is the
half of open question 9 this does not answer, and it is a different question — a retention policy
for work a person did ask for, which would narrow what a device that was not there can say about
it (session twenty-three's bug, from the other side). Left open on purpose, and now much smaller,
because the unbounded automatic supply was never the operator's runs.

**No new sweep and no new tick.** Both consumers are passes that already exist.

**`Origin` is not a permission or a scheduling input.** It must not reach bidding, placement,
capacity or the delivery plane. A triggered run is an *ordinary* run — that is ADR-0020's central
claim and the reason it changed no existing type — and the first time this field decides where
work goes, that claim stops being true.

### 6. What the walk found, which no test could

Two daemons, alpha holding the rule and refusing to host, beta hosting every occurrence. The
field travels: **beta's records all carried `machine_started = 1`**, learned purely by gossip
from a node beta has never been told anything about rules by.

Then the sweep reclaimed **nothing at all**, on the one machine it was written for. The guard
"no agent of ours is live on it" (ADR-0023 §2) was asked as `live.contains_key(&run_id)`, and
**`live` is not a map of running agents** — nothing ever removes an entry, so a key means "this
incarnation started that run at some point", which is true of every occurrence a peer has hosted.
`cancel` is the handle to the process and `release` clears it when the agent is gone; that is the
field `record_run` already reads to tell a live leg from a remembered one. The unit tests could
not see it, and the reason is worth keeping: **a fixture's runs are never actually run**, so
`live` is empty in every one of them and the guard was vacuously true.

With it fixed: **42 checkouts reclaimed in one pass, 22 MB → 2.2 MB**, and over 148 firings both
numbers stayed flat — 2 checkouts, 62 records on beta against alpha's 61, which is the same
five-minute quiet window on both. Before this, beta's were unbounded in each.

## Consequences

Good: the reclaiming built this session reaches the machine that had no way to know it was
accumulating anything. A phone that hosts a fleet's occurrences stops keeping a record and a
checkout for each one, and it does so by learning one bit rather than by being told about rules.

Bad:

* **A new field on the gossiped domain type**, which is the type this project changes least
  willingly. Justified by two measurements rather than by symmetry, and bounded by §5: it decides
  what may be *thrown away*, and nothing else.
* **A peer now deletes records and checkouts for runs it never ran.** Safe in a way the home node's
  version is not — a peer is not the arbiter, so a record it re-learns from a stale copy is inert
  (ADR-0021 §7 is about the node that *would* act) — but it is still a daemon deleting somebody's
  data on a timer, and the guards are the whole of what makes it right.
* **An older node in the fleet keeps everything.** Mixed-version fleets reclaim unevenly, silently,
  and correctly. Better than the alternative, and worth knowing before somebody measures one node
  against another and concludes the sweep is broken.
* **A rule that keeps failing now accumulates on every node**, not just its own — the failures
  ADR-0021 §2 keeps on purpose. Unbounded in the same way and for the same reason, and now with
  more copies of it.
