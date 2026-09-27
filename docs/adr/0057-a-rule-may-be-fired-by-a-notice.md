# ADR-0057: A rule may be fired by a notice, and an escalation is a delivery

**Status:** accepted · 2026-09-10 · **design ahead of code**, as ADR-0010, ADR-0011, ADR-0013 and
ADR-0019 were when they were accepted — nothing here is built or measured, so read it as intent
rather than as description · answers the one open question in phase 8's demo

## Context

Phase 8's demo ends with a sentence nothing in the tree can do:

> When the task exits non-zero, **a rule submits an agent run** that the fleet places on a machine
> that *does* have an agent.

Everything either side of it is walked. A node with no agent installed wins a bid for a task and
runs it; a schedule fires the cheap tier on a clock across two daemons; a trigger fires a task.
What is missing is the escalation, and it is missing because **nothing notices that a run failed
and starts another**.

The pieces that would have to be involved all exist, which is what makes this a decision rather
than a build:

* **The notice.** `notify::notable` projects a run's event log into the small typed subset worth
  interrupting somebody for (ADR-0010, ADR-0026), and `Notice::Failed` is precisely the sentence
  the demo names. It carries the reason.
* **The plane that fans one out.** `deliver::tend_deliveries` walks each route's cursor over the
  run and fleet logs, projects each entry, filters on `Audience` and `Notices`, writes an outbox
  row, and sends what is owed — with per-route queues, retries, an unusable-route report and
  dedup on `(sink, topic, seq)`. Every one of those is a thing an escalation would otherwise
  have to grow for itself.
* **The rule.** `offload when` stores a whole submission, fires one occurrence at a time, counts
  what it dropped, tags its occurrences and prunes them (ADR-0020, ADR-0021) — and since this
  session it fires either tier.

So the question is not *what could do this* but **which existing mechanism it belongs to**, and
the answer decides whether the fleet gains one concept or two.

## Decision

### 1. A rule may be bound to a **notice** instead of to a trigger's service

`offload when` binds a submission to the service of a trigger this node's owner nominated. It
gains a second binding: a **notice**, named by the same stable string `Notice::kind_name` already
hands to notification scripts (`failed`, `finished`, `overdue`, `asked`, `answered`).

Not a second mechanism. Everything below the firing is unchanged and shared: the `Submit` grant,
the one-occurrence-at-a-time guard, `note_rule_fired`, the occurrence tag, the pruning pass, the
`fired`/`dropped` counters and `offload rules`. What differs is only where the event comes from,
which is the same shape the `Work` split has one layer down — one dispatch point, and everything
either side of it about a *rule* rather than about what fires it.

The two bindings must be **spelled apart at the keyboard**, not guessed: `Service::Other(String)`
accepts any name, so a positional `offload when failed …` would silently mean "a trigger whose
service is called failed". A notice binding is a flag.

### 2. The firing source is the delivery plane, and a rule is a route

The pass that notices what is new already exists, and its route key is a **`TEXT` id** —
`sink_cursors(sink, topic)` and `deliveries(sink, topic, seq)`. So a rule is a route under a key
of its own (`rule:<id>`), and every property the plane already has applies to it with **no schema
change at all**:

* **A cursor per route**, starting at the *end* of the log, so a rule written this evening is not
  handed everything the node ever logged.
* **Dedup on `(sink, topic, seq)`**, so one notice fires a rule once however many passes see it.
* **Ordering by `noticed_at_ms`** rather than by sequence, because two logs number independently.
* **A queue per route**, so a rule whose occurrence is in flight does not starve a phone.
* **Retries and a report**, so "why did my escalation not fire" has the same answer shape as "why
  did my phone not buzz".

Delivering to a rule *is* firing it. That is the whole of the new code on this path: a route whose
send is a submission rather than a program.

**Why not a second scan.** A rule with its own cursor over the event log would be a second
implementation of the plane's hardest part, and this project has an entry for what that costs in
five different files. The plane's own history is the argument: the cursor-versus-outbox split, the
per-sink queue, the ordering fix and the `kind`-column-versus-projection fix were each a bug
found by measurement, and none of them is obvious enough to get right twice.

### 3. The loop guard: a notice about machine-started work never fires a rule

**The one thing this design must not do is escalate an escalation.** A rule bound to `failed`
fires an agent run; if that run fails it produces `Notice::Failed`; the same rule fires again;
for ever, one agent at a time, all night.

The guard is a fact already on the record: `Run::origin` (ADR-0024) is immutable, gossiped, and
says whether a **machine** started this run. So:

> A notice about a run whose origin is `Rule` is offered to sinks and **never to rules**.

Total, one comparison, and made of something no node has to be asked about. It also states the
policy plainly: *a machine's work does not start more of a machine's work.* An escalation that
itself fails is a person's problem, and the delivery plane will tell them — which is the division
of labour the whole notification plane exists for.

What that costs, and it is worth naming: **a chain of two escalations is unsayable.** "If the
watcher fails, have an agent look; if the agent fails, mail me" — the second half is a sink, which
is fine, but "if the agent fails, try a different agent run" is not expressible. Refused
deliberately: a bounded chain needs a depth counter on the record, which is a new gossiped field
with an owner and a merge rule (ADR-0005), and nobody has asked for one.

### 4. `Audience` is not asked; `Notices` is

`Audience` selects **routes**, and a rule is not a route to a person. `Audience::Nobody` means *do
not interrupt anybody about this run* — a preference about attention — and reading it as *do not
recover this run* would be one axis doing the work of two, which is the exact mistake ADR-0026
was written to fix. The precedent is in the pass already: the fleet's own log is offered to every
route "with no audience asked", because a device joining the fleet is not news a run gets to
scope. Whether a failure starts a recovery is not either.

`Notices` **is** asked, and it costs nothing: `Problems` and `Everything` both admit a failure, so
for the case this ADR is about the filter never fires — and keeping it in the path means there is
one path rather than two.

### 5. What the fired run is told

The notice's own `summary()` and the **run id** it is about, appended to an agent rule's prompt
under the heading a trigger's line already gets: *this is data from outside rather than an
instruction*. The id is what makes the escalation useful rather than decorative — `offload logs
<id>` is a real interface, served from wherever the run is, and an agent asked to look at a
failure needs to be able to read it.

For a task rule the notice is **not** passed, exactly as a trigger's line is not: ADR-0019 §1's
refusal of a submitter-supplied command line does not weaken because the text came from this
fleet rather than from a webhook.

### 6. Which node fires it

The node that **projected the notice**, which is the node that holds the run. A rule stays
node-local (ADR-0020, unchanged), so an escalation fires where the failure happened — and the
*placement* is the fleet's, through an ordinary bid round.

That is the demo's own sentence: the phone runs the watcher, the phone notices the failure, the
phone submits the agent run, and the fleet puts it on the desktop. An owner who wants the same
escalation wherever the work lands writes the rule on each node that could run it, which is what
`[[triggers]]` and `[[tasks]]` already ask of them.

### 7. Refused: a rule bound to a **fleet** notice

`Notice::Enrolled` and `Notice::PassphraseUsed` are about membership, not about a run
(ADR-0012's mitigations). Firing a run at them is a security response, and a security response
that submits work automatically is a decision with a much harder failure mode than a missed
notification. Refused at the keyboard with the reason, rather than silently accepted and never
fired.

## Consequences

Good: the demo's last sentence becomes true, and the fleet gains **one** concept — a rule with a
second kind of binding — rather than a second notification plane. The escalation is discoverable
in `offload rules` with its counters, because it *is* a rule; it is deduplicated and retried
because it *is* a delivery; and it needs no schema migration, no wire bump and no new gossiped
fact, because the route key was already a string and `Origin` was already on the record.

Bad, and none of it hidden:

* **`sink` is now a misleading column name** in two tables, and `Route`'s doc comment says "a
  route to a human". The rows are keyed by an opaque id, so nothing breaks; what is needed is the
  comment saying that a route's *destination* may be this node's own rule. Renaming a shipped
  column to fix a word is not worth a migration.
* **A chain of two escalations is unsayable** (§3), by choice.
* **The escalation fires where the run failed**, so an owner who wants it fleet-wide writes it
  more than once — the config-duplication cost `[[triggers]]` already has, arriving somewhere new.
* **A rule bound to `finished`** is expressible and is a foot-gun: every successful run fires it,
  including on a busy desktop. It is not refused — `Notices::Everything` exists for exactly that
  taste — but `offload when` should say what it will amount to, in the future tense, the way the
  audience and `--use` notes already do.

## Alternatives

**A sink whose program submits a run.** Works *today*, with no code at all: a sink is a nominated
program and the notice's summary is appended to its argv, so a two-line script calling `offload
run` is an escalation. Rejected as the fleet's answer for two reasons, and the first is fatal:
there is **no loop guard**, so the escalated run's own failure is a new notice with a new sequence
and the script fires again, for ever. The second is that what it submits is invisible — `offload
rules` cannot report a prompt that lives in a shell script, and "why did that agent run start" is
answered by reading somebody's `~/bin`. It stays the right answer for anything that is genuinely a
*route*, which is what a sink is for.

**A trigger program that polls.** The demo's literal reading: a nominated program notices the
failure and an ordinary rule fires. Rejected because the only interface such a program has is the
CLI — `offload ps --all`, `offload logs` — so it makes this project's own output a machine
interface and re-derives the notice projection in `awk`. The plane already computed the answer;
handing it to a program to reconstruct is the "second copy of the rule" this codebase spends most
of its comments avoiding.

**`offload run --task check --escalate "…"`**, a field on the spec. Attractive: the spec travels,
so the node that fails the run already holds the instruction, and there is no matching and no new
firing source. Rejected because it is per-*run* where the need is generic — every occurrence of
every schedule would repeat it — and because it puts a whole submission inside a `RunSpec` that
already has a prompt and a repo of its own, which is the one-field-two-facts shape this project
has now fixed four times. Worth revisiting only if somebody wants an escalation that differs per
submission.

**A depth-bounded chain** instead of §3's flat refusal. Rejected for now: the counter has to live
on the record to survive migration, so it is a new gossiped field with an owner and a merge rule
(ADR-0005) — and the case for it is hypothetical while the case against unbounded escalation is
an agent running all night.
