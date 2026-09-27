# Rules, occurrences and reclaiming what they leave

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/rules-and-retention.md`, same order.

The premise every one of these breaks: a rule fires all night, so **there is nobody who will read
the output**. Any mechanism justified by a person stops holding here.

- **A rule's failed occurrence is picked back up behind the rule's back.** Auto-resume is a second
  route past "one occurrence at a time": `rule_run_in_flight` reads `rules.last_run`, which by then
  names the *newer* occurrence. A firing withdraws the previous occurrence's recovery watch
  (ADR-0027) — so a nightly rule still resumes and a fast one does not.
- **…and a peer retried them while the rule's node reported perfect health.** ADR-0027's withdrawal
  is a no-op when the occurrence ran elsewhere. **Recovery of a machine-started run belongs to the
  machine that started it**: `origin` travels, `runs.rule` deliberately does not. Checked *before*
  attendance, because an occurrence is unattended by construction.
- **`Supervisor::cleanup` is deliberately manual because somebody will read the worktree** — and a
  triggered run has nobody. `reclaim_occurrence` is the automatic half, bounded by rules from
  elsewhere: only when the checkout holds nothing **uncommitted**, only where the checkout *is*,
  and asked of **git** rather than the turn-boundary-stale summary beside the run.
- **A prune that gets one chance per record mostly does not happen.** Everything between a spent
  occurrence and `delete_run` is a *temporary* no. So a firing asks about **every** occurrence of
  its rule — that is what the `runs.rule` tag is for, and why `last_run` was not enough.
- **`live` is not a map of running agents.** Nothing removes an entry, so `contains_key` means
  "this incarnation started that run at some point". `cancel` is the process handle; `release`
  clears it. A guard that reads a map only production populates is a guard no fixture can exercise.
- **Reclaiming reaches only as far as the fact that justifies it travels.** A `RuleId` is one
  machine's name for its own thing, so what travels is the smaller fact that a *machine* started it
  (`Run::origin`). `runs.rule` stays local and `runs.machine_started` travels, in one `INSERT` with
  **opposite** `ON CONFLICT` rules.
- **Nothing removes a worktree when a run leaves.** In the sweep, **"not the holder" is a trap** —
  it is true of every finished run, so a sweep built on it deletes the checkout `cleanup` is manual
  for. `RunProgress::by` is the durable form, and unstamped means unknown rather than somebody else.
- **A node does not learn of its own run from somebody else.** A record naming us as `home` for a
  run we do not have was made here and deleted here, so `merge_run` refuses it — otherwise a pruned
  record comes back from a laptop that was away, held by us, with no agent behind it.
- **…and a quiet period buys the ability to re-ask, not safety.** Where there is no future firing
  the clause protects nothing: `offload unwatch` left 101 records behind. `Prune::{AtAFiring,
  Finally}`. A delay justified by "we will check again later" is wrong wherever there is no later.
- **The outbox has two halves, and only one has a row.** A row is created by a *scan*, so an event
  no route has reached yet is news with nothing to point at. `deliver::scanned_to` is the other
  half — the lowest cursor among routes that **exist right now**; a route with no cursor holds
  nothing back.
- **A bystander's record is the index that makes the log reachable.** `logs` resolves the id
  locally and forwards to `RunProgress::by`; with no local row there is nothing to resolve. It is
  also why a node pruning somebody else's record would delete and re-learn for ever. Before
  sweeping something that looks like residue, find out what reads it.
- **…and what was missing was never a sweep, it was that nothing said so.** `offload status` prints
  the kept-record count, and the checkout-with-no-run-record count. Counts, not sizes: `status` is
  the cheapest command in the CLI.
- **A "does not reach" note is a claim with a date on it.** ADR-0023's stated gap was closed by
  ADR-0024 two commits later, and the sentence survived in four places. The commit that closes a
  gap is not the commit that will remember to delete the note.
- **A prune is node-local, and gossip is not.** A peer learns every occurrence and nothing there
  will ever remove one. Unbuilt on purpose: it is a retention policy for *every* run, and it
  changes what a machine that did not run the work can answer.
- **ADR-0020 §3's guard was 7ms from the submission it authorises.** `fire` asks
  `rule_run_in_flight`, then awaits `reclaim_occurrence` — `git status` (3.0ms) plus, on a clean
  checkout, `git worktree remove --force` and a `prune` (4.1ms) — and only then submits. ADR-0027's
  `stop_recovering` above it fences the *machine* that would revive the previous occurrence; nothing
  fences the person, and `Supervisor::resume` accepts a `Failed` run by name. Measured with the
  revival landing 1ms into the git call: **the firing submitted a second occurrence and made it the
  rule's `last_run`** — "two agents on one repository by a new route", which is §3's own sentence,
  reached from the other side. `in_flight` is asked twice now; a store error reads as *dropped*,
  because not knowing whether an occurrence is running is not permission to start a second one.

## Schedules (ADR-0019 §3, ADR-0056)

A rule and a schedule look like the same thing and differ in the one way that changes everything:
a rule is **node-local** because a trigger is a program one machine's owner nominated, and a
schedule is **gossiped** because a clock is everywhere. Every entry below is that difference
arriving somewhere.

- **The occurrence's record is not enough to say a tick has fired, because the record is deleted.**
  A scheduled occurrence is machine-started, so `prune_spent_records` reclaims it about an hour
  after it finishes (ADR-0021, ADR-0024) — which for a **daily** schedule is twenty-three hours
  before its tick ends, so the schedule fires roughly hourly. Found by writing the ADR's own
  §2 and then reading the retention pass. What fixes it is a **node-local** high-water mark
  (`schedules.last_tick_ms`), outside the row's `ON CONFLICT` list the way `runs.rule` is: the
  mark stops *this* node re-firing, and the record stops a *second* node firing what this one
  already served. Two checks, two failures, neither covering the other.
- **A schedule is not due for the tick it was born in.** The tick containing the moment of
  creation *started* before it, so the first pass after `offload every 15m` fires immediately and
  then again at the next boundary — two occurrences eight minutes apart from something that says
  every fifteen. Measured on a daemon, one command apart: `first tick in 36.9s` at the keyboard
  and an occurrence in the log five seconds later. `Schedule::fires_tick` compares against
  `created_at`, which is immutable and gossiped, so a peer that learned the schedule a second
  later declines the same occurrence rather than firing the one the creator refused.
- **A refusal must not consume the tick, and a retry must not be unbounded.** The first build
  marked the tick before submitting — the ordering that cannot double-fire — and so an occurrence
  nobody would take spent its period: measured on two daemons, where the successor fired
  correctly into a fleet whose only live node was still on probation. The mark moved after the
  placement, bounded by a **dozen attempts** per tick. And the bound has to count *attempts*, not
  elapsed time: a window measured from the start of the tick refuses to serve a daemon that
  started up in the middle of one, which is exactly when firing is right.
- **A gossiped set needs a tombstone.** `removed_at` on the row, and removal is an UPDATE: a
  deleted row comes straight back from the next peer that gossips it, and the schedule then fires
  for ever on whichever node was last to hear. Same shape as `Revocation` and the same argument —
  the definition need not travel, the removal must. Tombstones are kept, and `offload schedules`
  says so plainly rather than hiding a row somebody may wonder about.
- **Two nodes firing one tick is one *record* and would be two *agents*.** The derived id
  (`Schedule::occurrence_id`) is the net under a transiently doubled steward, never the mechanism:
  exactly one node fires, by `ClusterView::steward_of` — which is `arbiter_for`'s rule, factored
  rather than restated. Placement still goes through one round per run per node (ADR-0050).

- **A rule can fire either tier, and the *event* stops at the boundary.** `offload when <service>
  --task <task>` fires a nominated program; an agent rule appends the trigger's line to its prompt
  under a heading that says it is data, and a **task is not told the line at all**. That is
  ADR-0019 §1 rather than an omission: a trigger's output is text from outside, and passing it as
  `argv` to a program the owner nominated is the submitter-supplied command line that ADR refuses
  in the strongest terms it uses. If the payload is ever needed, the channel for it (stdin, or one
  named environment variable) is a decision with its own reasoning.
- **`SubmitTaskRequest` had no `origin`, which was invisible until a machine could fire one.** The
  agent request has carried ADR-0024's fact since it existed; the task request did not, so a
  rule-fired task would have been recorded as an **operator's** — nothing would ever reclaim its
  record, and `cleanup`'s premise that somebody will read the worktree would have been applied to
  a run nobody typed. Found by the compiler while splitting `RuleSpec`, which is the good case:
  the field is `#[serde(default)]` and stamped by the daemon at the firing, so nothing trusts what
  was stored.
- **A rule's stored shape is a storage format, and no version number covers it.** A rule is a
  whole `RuleSpec` as JSON in `rules.request_json`, so splitting the type changed what a daemon
  reads back from its own disk — with no schema migration to hang it on, because the table did not
  move, and with the wire version protecting nothing, because a rule is never gossiped. Without a
  compatibility deserializer, upgrading loses every standing instruction on the machine. Same
  hazard `RunSpec` had one session earlier, and the fixture that pins it is **hand-written** for
  the same reason: one generated by re-serialising today's type changes shape in lockstep with the
  code it is meant to hold still.
- **A precondition is a refusal on one command and a note on the other, and the fact underneath
  is one function.** `offload run --task push` is refused at the keyboard; `offload when
  --task push` is *written* with a warning, because a rule fires for months and the machine that
  will nominate the program may not have enrolled yet (ADR-0032, ADR-0036). Both read
  `nominates_task`, which asks this node's config and the fleet's advertised `Role::Execute`
  capabilities. Walked, including the promise the note makes: `offload rules` counted the refused
  firings under DROPPED with the round's own sentence.
- **A schedule survives its device being *away*, not its device being *forgotten*.** ADR-0056's
  headline claim is the difference between this and `cron`, and it holds through a home node that
  is `Dead` — the successor rule hands the tick to the lowest-id live node. It does **not** hold on
  a node that has restarted since the home went away: a fresh daemon's view has no peers, so
  `steward_of` answers `None` and nothing fires. That is the right answer (a daemon that has met
  nobody must not begin firing the whole fleet's schedules) and it is self-healing — measured, the
  schedule resumed on its own once the home returned on the seed backoff, with no intervention.
  What it is not is invisible: see the entry in `reports-and-cli.md` about the `bool` that called
  it "fired elsewhere".
- **…and any node may write a schedule's tombstone, which is what makes a retired home
  removable.** ADR-0056 §5 said the home owns `removed_at`; the code never checked, and the code is
  right — the merge is a minimum over a monotonic field, so it converges whoever sets it, and a
  tombstone only the home may author cannot be written at all once that laptop has been sold.
  Measured: `offload unschedule` on bravo for a schedule homed on alpha reached alpha in eight
  seconds and stopped it everywhere. The ADR carries an amendment; the precedent is `Revocation`,
  which travels for the same reason.
- **A precondition added for `offload when` was never added to `offload every`, and the argument
  for it is stronger there.** The entry above about "a refusal on one command and a note on the
  other" has a third caller nobody wrote: `offload every 1m --task nosuch` was accepted **in
  silence** while `offload when --task nosuch` warned, and `offload every`'s own help promises the
  schedule does *not* die with the device you type it on. Measured: the bid round refuses at every
  tick for ever (`ineligible: has nosuch as execute`) and `offload schedules` prints `nothing
  fired from here yet` — the same sentence it uses for a schedule that has not reached its first
  tick. The `notify`/`reach` notes were missing beside it for the same reason.
- **…and the shared thing is the predicate, never the sentence.** Borrowing `task_note`'s wording
  would have pointed somebody at *"`offload rules` counts those under DROPPED"* — a report that
  will never mention a tick, because **nothing counts a refused tick**. `nominates_task` and
  `missing_resources` are what must not diverge; the consequence differs per command and is
  properly worded per command, which is the same split ADR-0032 already makes between `offload
  run --task` (a refusal) and `offload when --task` (a note). The schedule's note says the gap out
  loud rather than papering it: *"Nothing counts a refused tick, so `offload schedules` will go on
  saying nothing has fired from here."*
- **When a command grows a second tier or a second front door, grep for every caller of the
  predicate, not for the type.** This was found by typing the command, not by reading — the
  comment directly above `Request::Every` is *about* this class of omission on its neighbour and
  did not prompt anybody to check the arm underneath it.
- **The two removal commands prune differently, and the asymmetry is correct.** `offload unwatch`
  prunes its rule's occurrences *immediately* and with the quiet period disabled
  (`Prune::Finally`), because a rule is node-local, its occurrences carry the local `runs.rule`
  tag, and there is no future firing that could ever ask again. `offload unschedule` prunes
  nothing and leaves it to the fifteen-minute retention pass in `main` (`rule = None`,
  `Prune::AtAFiring`) — which is right for the opposite reason: a schedule is **gossiped**, so a
  peer is still teaching those occurrence records for `GOSSIP_TAIL`, and a record deleted inside
  that window comes back untagged and therefore unprunable for ever. Here the quiet period buys a
  real re-ask, because the pass recurs whether or not anything fires. Walked: both removal
  commands, one daemon; `unwatch` took 115 records to 14 in one call, and the tombstoned
  schedules' occurrences were left for the timer on purpose — **and the timer was then watched
  rather than trusted**: 23 records to 8 at exactly fifteen minutes after daemon start, the 8
  being 7 `failed` occurrences (kept by ADR-0021 §2) and 1 operator-submitted run that
  `spent_occurrences` never considers. Do not "make them consistent".
- **A rule or a schedule bound to a task nobody can *run* is a different silence from one bound to a
  task nobody nominates, and only the second was reported.** `nominates_task` did not read the
  capability's `authenticated` bit, so `offload when --task` and `offload every --task` accepted a
  binding to a `[[tasks]]` entry whose program is missing without a word, and refused every firing
  for ever. The entry is in `refusals-and-instructions.md`, with the measurement — what was wrong
  was the sentence, and it was found by typing the control beside it.
