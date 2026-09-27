# ADR-0026: A run says which news is worth interrupting somebody for, as well as who to tell

**Status:** accepted · 2026-08-27 · completes ADR-0010's audience · supersedes nothing

## Context

ADR-0010 gave a run an `Audience`: *who* its news is for, named by service, `Everyone` by default.
ADR-0020 then made runs arrive **by themselves, for ever** — and the two had never been walked
together. Every trigger walk so far has run on a node with no sinks configured at all, so the
delivery plane was present in the design and absent from the measurement.

With a sink configured, a rule firing every three seconds does this:

```
13 firings  →  11 notifications
06:08:42 | finished after 2 turn(s), $0.0005
06:08:47 | finished after 2 turn(s), $0.0005
06:08:52 | finished after 2 turn(s), $0.0005
```

That is a phone buzzing every three seconds, all night, to say nothing happened. It is the exact
thing CLAUDE.md already names about a sink's cursor — "which is how somebody learns to turn
notifications off" — arriving through a different door.

The escape hatch makes it worse rather than better. `--notify nobody` on the *same rule failing
every firing*:

```
11 firings, every one failed  →  0 notifications
```

So a standing instruction had exactly two settings, and both were wrong:

* tell me every three seconds that nothing happened, or
* tell me nothing, **including that it broke** — which is the one thing a watcher exists to say.

There is no third value, and the reason is structural rather than an oversight: `Audience`
selects **routes** — reach me by push, not by mail — and is asked per capability for exactly that
reason (ADR-0010's amendment). What a watcher needs is a selection over **kinds**. One axis was
being asked to do the work of two.

## Decision

### 1. `RunSpec::notices` — the kinds axis, beside the routes one

```rust
pub enum Notices { Everything, Problems }
```

`Problems` is everything except the run finishing. It is *not* "only failures": a missed deadline
and a question are requests for attention too, and an unanswered question blocks the run mid-turn
until its patience runs out (ADR-0017), which is the last thing to be quiet about. The line falls
in one place because there is only one notice in the projected set that is not somebody asking for
attention.

Beside `notify` rather than folded into it, because they compose: *"reach me by push, and only
when something is wrong"* is one sentence with two answers in it. On the **spec**, for `notify`'s
reason verbatim — the node that delivers is usually not the node that took the request, so a
selection that did not travel is one a migration silently widens. Not in `SpecEdit`: two editable
fields share one counter on purpose, and a third belongs to an ADR rather than to whoever needs it
first.

### 2. Two values, not a set of kinds

The question a person actually has is "tell me when it needs me". A checklist of five notice kinds
is a configuration surface for a decision nobody wants to make twice, and every additional value
is a way for somebody to accidentally silence the failure. Two values, one of which is the old
behaviour.

### 3. The default differs between a submission and a standing instruction

`offload run` defaults to `Everything`: you typed it and walked away, and *"is it done"* is the
question this plane exists to answer without you checking all evening. That is unchanged.

`offload when` defaults to **`Problems`**, and that difference is what a watcher *is*. A rule fires
on its own, possibly all night; its value is that it is quiet until something happens. `--notify-on
everything` opts back in, for a rule that fires rarely enough to want a heartbeat.

A default that differs from the neighbouring command's has to be **said, once, where it is
chosen** — `offload when` prints which way it went, and `offload rules` prints `reports problems
only` against a rule that has it. A rule written months ago whose author has forgotten the setting
is precisely the case where "no notification" and "no failure" look identical.

### 4. Decided where the news is *noticed*

The filter sits beside the audience check in the scan that fills the outbox, not at send time —
ADR-0010's rule, for its reason: an outbox row is a promise to deliver, so news a run never wanted
must never be *owed*. The cursor still advances past it, because a cursor bounds the scan rather
than recording what was sent.

It reads the event log's denormalised `kind` column, which the scan already filters on and never
decodes — the content is re-derived at send time. `notify::GOOD_NEWS` is the one string the
projection and the filter share, with a test whose only job is to fail if `Notice::Finished` stops
answering to it: a rename would otherwise turn `Problems` into `Everything` in silence, and no
test would say so.

### 5. Not `Origin`, deliberately

The obvious cheaper fix is for the delivery plane to notice that a run was started by a rule and
be quiet about its successes. ADR-0024 forbids exactly that — origin "must never reach bidding,
placement, capacity or **the delivery plane**, because a triggered run is an ordinary run" — and
the prohibition is right here rather than merely binding. Two reasons:

* Some watchers genuinely want the heartbeat, and a bit the plane reads takes the choice away from
  the rule's author. A field on the spec gives it to them.
* An operator run wants this too. "Don't buzz me when it works, only if it breaks" is a reasonable
  thing to say about a run you typed, and a fix keyed on origin could not express it.

So the decision is made by the *author*, at the moment they write the rule, and travels as an
ordinary field. The plane learns nothing about where the run came from.

### 6. No wire bump

`#[serde(default)]`, and the default is `Everything` — today's behaviour. A record written by an
older build reads as "tell somebody", which is the direction the whole plane defaults in: a
duplicate is a nuisance and a silence is the failure. A rule stored before this existed keeps
notifying on every firing, which is what it has been doing, and `offload rules` says so by *not*
printing the quiet line. Verified on a rule written by the previous build.

## Consequences

Good: the case ADR-0020 created has a setting that fits it, and it is the default. Measured on one
rule, same trigger, same sink, only the agent changed:

```
succeeding, --notify-on problems (the new default):  37 firings  →   0 notifications
failing,    --notify-on problems (the same rule):    14 firings  →  14 notifications
```

Before: 13 firings → 11 notifications for the succeeding rule, and 0 of 11 failures for the only
setting that could quieten it.

Bad:

* **Two fields that both look like "notifications".** `--notify` and `--notify-on` are one letter
  and a hyphen apart, and somebody will reach for the wrong one. Mitigated by each saying what it
  did — `offload run` already reports its audience, and `offload when` now reports both — and not
  by renaming, because `--notify` is what an operator has typed since session ten.
* **A rule's successes are no longer in the outbox at all**, so `offload sinks` counts fewer
  deliveries for a fleet that has not changed. The events are still in the run's log, which is the
  distinction `Audience::Nobody` already draws: this decides who is *interrupted*, never what is
  recorded.
* **`Problems` is a judgement about which notice is not a request for attention**, and it has
  exactly one member today. A new notice kind has to decide which side it is on; the test over
  `NOTABLE_KINDS` makes that a decision rather than a default.
* **A watcher that fires and succeeds is now silent**, which is indistinguishable from a watcher
  that is not firing. `offload rules` is the answer — `fired`, `dropped` and `kept` are all there
  — and it is one more reason that command prints what it does.
