# ADR-0019: Work that is not an agent run is a program the owner nominated, fired by a schedule

**Status:** accepted · 2026-08-24 · answers roadmap open question #8 · **amended 2026-09-12**,
where §1's placement clause turned out to be one word wrong once it was measured on a daemon ·
written as **design ahead of code**, as ADR-0010, ADR-0011 and ADR-0013 were when they were
accepted — nothing here was built or measured at the time, so read the body as intent rather than
as description. All seven items are built now.

## Context

The scenario, from the sketch at the end of `docs/sessions.md`: a phone joins the fleet, has no
agent, but can watch an API and report when a response changes. It should take that work always,
and heavier work only while charging.

**What already fits, stated as the code now is rather than as the sketch guessed.** Some of this
has been built since the sketch was written and the difference matters — two of the four gaps it
named are closed.

* **Capabilities versus policy.** Exactly right, and it is the split the whole answer rests on:
  the phone *lacks* `agent=claude-code`, which is a fact, while "only when charging" is its
  owner's policy.
* **`Restartability::Idempotent`** describes a watcher perfectly — no transcript, no workspace,
  re-runnable from its spec — and migration becomes a reschedule.
* **`Demand` exists, and capacity and pressure already honour it** (ADR-0013). `Capacity::room_for`
  charges `Demand::shares` and `needs_idle_percent` scales the load threshold, so "four light runs
  in the space of one normal one" is shipped behaviour. The sketch listed this as missing.
* **The reporting half is settled** by ADR-0010, and built: a device that hosts nothing but can
  reach a person is already a useful fleet member.
* **`Service::Schedule` and `Role::Trigger` are already in the vocabulary** — "something that fires
  on a clock rather than on an event" and "something arrives and creates or wakes work" — declared
  by ADR-0011 and used by nothing.
* **The domain is agent-free.** `Run`, `RunState`, `Lease`, `Epoch`, bidding, arbitration and the
  migration policy never mention agents.

**What does not fit.** Four things, and the third is the only hard one.

1. **`RunSpec` assumes an agent.** Six of its fields are agent-specific (`agent`, `model`,
   `prompt`, `workspace`, `permission_mode`, `allow`, `max_turns`) and the rest are not. A watcher
   has no prompt and no workspace.
2. **The *supervisor* is not agent-free**, whatever the domain is. It holds a concrete
   `ClaudeCode`, and the `Agent` trait it could dispatch on has two methods (`kind`, `version`),
   neither used for dispatch. So "the run machinery is already agent-agnostic" is true of
   `offload-core` and false of `offload-node`, which is the difference between a small change and a
   new seam.
3. **Recurring work has no home**, and a schedule is a gossiped fact, so ADR-0005 wants an owner
   and an arbitration rule for it.
4. **The owner's gates are all-or-nothing.** `accept`, `min_battery_percent` and `allow_metered`
   do not consult `demand`, while `room_for` and `needs_idle_percent` do — which is exactly why
   "take the light watcher always, heavy work only while charging" cannot be said today.

## Decision

### 1. The work is a program the node's owner nominated

`[[tasks]]` in node config: an id, a `Service`, a command, optional `env`. A run asks for it **by
service** — `offload run --task watch-api` — never by command, and the command never leaves the
node. A peer learns that this device *has* a `watch-api` task and never how it is invoked.

This is the sink rule (ADR-0010) and the resource rule (ADR-0011) applied a third time, for the
third direction, with the same justification: *the service is the owner's declaration and the
command is only how it is invoked*, because the one thing a general-purpose machine can honestly
verify is that a program exists. Three uses of one rule is a good sign it is the right rule.

Two things follow, and they are the reason this is the design rather than a convenience.

* **The outbound network access is the owner's grant, not the submitter's.** The sketch predicted
  that non-agent work would need "an allowlist story of its own" for outbound HTTP. It does not:
  nominating the program *is* the grant, and the submitter never says where to connect. A gap that
  closes by choosing the right shape is better than one that closes by adding a mechanism.
* **A submitter cannot execute arbitrary code anywhere in the fleet.** Refused explicitly: a
  submitter-supplied command line, and a submitter-supplied URL. That is the allowlist rule read
  together with the sink rule — a repo may say "run my tests", not "give me `sh`" — and a
  workload kind that took a command from the submission would be that hole with a new name.

`Constraint::HasService` already asks the question that matters at the keyboard: does *anybody*
have this task (ADR-0014, ADR-0011's amendment). Placement is not pinned by it.

### 2. `RunSpec` splits: shared fields, and a `Work` enum

```
Work::Agent { agent, model, prompt, workspace, permission_mode, allow, max_turns, ask }
Work::Task  { service, args }
```

Shared and unchanged: `constraint`, `restartability`, `priority`, `queue`, `deadline`, `demand`,
`notify`, `resources`.

**Not an `Option<Task>` beside the agent fields.** One field that is two facts is the mistake this
project has now fixed three times — `LogKind::Failed` borrowed for a failed capture, one signal
carrying both "stop the agent" and "the run is over" (`Halt`), and a run's *position* sharing a
merge rule with its *spend* (ADR-0005's second amendment). An enum makes the impossible
combinations impossible rather than merely unwritten.

Four consequences worth settling here rather than discovering:

* **`Restartability::Resumable` is invalid for a task** and is refused at submission. A task has
  no transcript, so "resume mid-conversation" has no referent; discovering that at migration time
  means the migration already failed, which is the mistake open question #4 made about
  `Portability::NodeLocal`.
* **No checkpointing, and that is not a special case.** A task has no turn boundary, so ADR-0004's
  only-safe-place question does not arise, `Checkpointing` is never entered, and nothing is
  captured. Last session's split makes this coherent instead of awkward: a task has no *position*
  (no turn, no worktree) and no *spend* (no model bill, no denials, nobody asked), so both halves
  of `RunProgress` are legitimately empty rather than zero-that-means-unknown.
* **A task's identity for concurrency is its service, not an account.** `held_for_agent` and the
  account fingerprint are agent concepts — there is no rate limit to share. What bounds a task is
  the node's `Demand` budget, which already reads the spec's own declaration.
* **A task's log is its output.** Stdout and exit status, in the run's event log, which is what
  `offload logs` already serves from the holder. No new plane.

### 3. Recurring work is a schedule that fires idempotent runs

Not a long-lived run that sleeps between polls. A sleeping run holds a lease and a slot for hours
to do a second of work, and it makes "the phone is asleep" indistinguishable from "the run is
between polls" — the same conflation ADR-0007 refuses between unreachable and dead. Firing short
`Idempotent` runs lets a sleeping phone simply not bid, which is existing machinery doing the job.

The hard part is the one the sketch named and did not solve: a schedule is a gossiped fact, so
ADR-0005 wants an owner, and **two nodes firing one tick is two runs.**

* **Owner: the node the schedule was created on**, with arbitration's own successor rule — that
  node while it is available, else the lowest-id available node (`arbiter_for`). Same rule, same
  reason, no election.
* **That is not enough**, because the successor rule can transiently name two owners. It is the
  few-missed-probes case that produces two arbiters (ADR-0002's amendment), and here there is no
  record yet to fence against: fencing orders writes to an existing run and says nothing about
  *creating* one.
* **So an occurrence's `RunId` is derived from `(schedule, tick)`.** Two firings of one tick are
  then the same record, and `merge_run` settles them as one run rather than as two. It is
  computable by every node from the schedule alone, offline, with no message — the same property
  the equal-epoch tiebreak and `winner()` are chosen for.

  Keeping the UUIDv7 shape is a requirement, not a nicety: bytes 0..6 of a v7 id are a 48-bit
  millisecond clock, and the twelve-character display prefix *is* that clock (CLAUDE.md, and
  `byte_id!`). So the tick's unix milliseconds go in bytes 0..6 — which is what a v7 timestamp
  already means — and the remaining bytes are a digest of the schedule id. The clock property
  survives, the prefix keeps meaning what it means, and the id is deterministic.

  This is the one place in the design where creation *converges* rather than being owned, and the
  reason that is acceptable here is worth stating: a duplicate **record** merges, and a duplicate
  **agent** does not. Placement can never work this way.
* **A missed tick is not made up.** A phone asleep for six hours does not wake to six runs. A
  watcher's value is the current state, and a backlog of stale occurrences is exactly the
  notification storm ADR-0010's audience rules exist to prevent. Catch-up is refused; if somebody
  needs it, that is a decision with its own ADR and its own answer for what a stale occurrence
  even means.

### 4. The owner's gates gain the demand axis, defaulting to unchanged

`accept`, `min_battery_percent` and `allow_metered` get a *light* form, unset by default, so a
fleet that says nothing behaves exactly as it does now — the `budget_shares: None` precedent, and
the reason it matters here is that these are the gates people have already configured.

**Refused: making `Light` bypass the gates by default.** `Demand` is declared by the *submitter*,
and ADR-0013 already treats it as a hint whose harsher reading wins at admission. A gate keyed on
a submitter-supplied hint is a gate a submitter can talk their way through, so the light form is
the *owner* saying "I trust light work here" — never the run asserting it. A phone that said "when
charging" must not start working on battery because somebody typed `--demand light`.

## Consequences

Good: one scheduler, not two. Bidding, leases, epochs, arbitration, migration, the delivery plane
and the audit log all apply unchanged, and a phone that cannot host an agent becomes a device that
can host *something* — which is the difference between a fleet member and a notifier. `NoBid`
still explains itself, so "why didn't my watcher run" has the same answer shape as "why is my run
still pending".

Bad, and none of it small:

* **`RunSpec` is a wire and a schema change**, touching every construction site — and there are
  many, since fixtures build one by hand in a dozen tests. This is the largest single edit the
  project has taken on since capabilities became instances.
* **Two kinds of run means two shapes in every output.** `offload ps` shows turns and cost; a task
  has neither. The honest version of that column is empty, not zero, and every reader has to know
  the difference.
* **A task's failure has no transcript.** Its log is whatever it printed, so a badly-behaved
  program is a badly-diagnosed run. Nothing here can fix that, and pretending otherwise by
  inventing structure over stdout would be the "never reimplement an agent" rule broken from the
  other end.
* **The derived `RunId` costs an invariant.** "Every run id is minted once, by the daemon that took
  the operator's command" is currently true, and it is what made three of `storm.rs`'s early
  "invariant violations" legible as harness bugs. After this, ids have two provenances, and any
  future simulation has to know which.

## Alternatives

**Built-in workload kinds** — `Work::HttpPoll { url, interval }` as a first-class variant.
Rejected: it puts an HTTP client, a change-detection rule and a retry policy inside an
orchestrator, which is "never reimplement an agent" arriving in a different costume, and it takes
a URL from the submitter.

**A long-lived run that sleeps between polls.** Rejected above: it pays a lease and a slot all
night for a second of work, and it makes a sleeping device indistinguishable from a waiting run.

**Cron on each node, outside Offload.** Simplest by far, and it is what somebody will do anyway.
Rejected as the fleet's answer because it has no fleet: no view of who could run it, no migration
when the device leaves, no `explain` when it does not fire, and no delivery plane when it wants to
tell you. Those four are the whole product.

**Triggers only, and no non-agent work at all** — the phone is a `Role::Trigger`, and the work it
starts is an ordinary agent run somewhere capable. Genuinely attractive, and it is *half* of the
right answer rather than a rejected one: `Role::Trigger` is declared and unbuilt, "something
arrives, work starts" is the other way recurring work reaches this system, and it needs none of §2.
**If only one of the two gets built, build this one** — it is smaller, it changes no existing type,
and it serves the case where the fleet does have somewhere to do real work. What it does not serve
is the case in the scenario: work small enough that involving an agent is absurd, on a device that
cannot host one.

## Amendment, 2026-09-12: the clause is `ServiceAuthenticated`, not `HasService`

§1 above ends with *"`Constraint::HasService` already asks the question that matters at the
keyboard: does **anybody** have this task"*. That sentence was written as design ahead of code,
and it is wrong by one word: `HasService` asks whether anybody **nominated** one.

A `[[tasks]]` block is three fields, and one of them is a path. So a node whose `command` names a
program that is not on it satisfies `HasService` exactly as well as a node that can run the thing.
Measured on one daemon, on the default configuration:

```text
$ offload run --task nightly
run 01a0942db7e8
$ offload ps --all
01a0942db7e8   failed   task   -  -  -  nightly
           └─ agent: the program for task `nightly` was not found: /tmp/ow81/nightly-report.sh
```

Accepted, placed here, failed a millisecond later — which is the outcome ADR-0014 exists to
prevent, and which the *no-cluster* arm of this same door had already been fixed for one cause
over (a service nobody nominates at all). On a fleet with more than one node it is worse than a
wasted run: the node that cannot do the work bids against the node that can, and wins as often as
the score says.

**The bit was already there.** §1's own argument is that *the one thing a general-purpose machine
can honestly verify is that a program exists*, and `task_capabilities` has set
`Capability::authenticated` from exactly that, on every probe, since the tier was built. Nothing
read it. `Constraint::ServiceAuthenticated` — a variant that existed and that nothing constructed
— is the clause that does, and it is what the **resource** tier one ADR earlier already places on
(`CanUse` is `is_resource() && authenticated`). Two applications of one rule, and the newer one
dropped the bit.

So the amendment is one word in one clause, and then every reader of the weaker fact:

* `build_task` builds `ServiceAuthenticated { role: Execute }`, which is what a submission, a
  rule's firing and a schedule's occurrence all go through.
* `task_refusal` — the fleet-of-one door, which has no round to ask — reads the capability rather
  than the config, and answers three ways rather than two (`task::Nomination`): nothing nominated,
  nominated and unrunnable, ready.
* `nominates_task` — which `offload when --task` and `offload every --task` warn from — asks
  `authenticated` on this node *and* on every peer. Without it both commands accepted such a rule
  and such a schedule **in silence**, refused at every firing for ever; the control, a service
  nobody nominates, got the whole paragraph from each.
* `offload status` grows a `task` line beside the `resource` line that has always been there, so
  the owner can see a broken nomination on the command they run.

**What does not change.** Placement is still not pinned to a device, and the command still never
leaves the node: what travels is the service and one bit. Nor does the wire —
`ServiceAuthenticated` is an existing variant of an existing enum, so a peer that has never heard
of this change reads the spec and evaluates it correctly.

**Two residuals, stated rather than left.** `authenticated` is a snapshot from the last probe, so
a program installed thirty seconds ago is refused for up to thirty more (ADR-0048's residual, met
here). That is the honest direction to be wrong in — the alternative is placing work on a node
that cannot do it — and the lever is the probe cadence, not this clause. And a **schedule created
before this change** keeps the constraint its `RunSpec` was written with (ADR-0056 §3: the spec is
complete at creation), so its occurrences still place on the weaker clause until it is re-created.
Correct, and worth knowing before reading such a row as a bug.
