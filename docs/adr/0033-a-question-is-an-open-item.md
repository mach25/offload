# ADR-0033: A question is the one notice that is an open item, so the plane has to close it

**Status:** accepted · 2026-08-27 · completes ADR-0017's delivery half · amends ADR-0026's
`Problems` filter · supersedes nothing

## Context

`--ask` (ADR-0017) and rules (ADR-0020) had never been walked together, and both are justified by
a *person*: a question waits for one, and a rule fires because nobody is there. Walked on one node
with a working push route, a trigger every three seconds, `--permission ask --ask=3`:

```
t=180s   1 firing   94 dropped   ask: 16.2s left    1 notification
t=210s   2 firings 103 dropped   ask: 4m46s left    2 notifications
t=240s   2 firings 113 dropped   ask: 4m16s left    2 notifications

kinds delivered:  2 × "asked"      ← and nothing else, ever
```

Two things came out of it, both in one notice.

### 1. The question was sent with no clock, which is the promise its own doc comment says it cannot make

`Notice::NeedsDecision`'s doc comment reads:

> the only one with a deadline of its own: the run is mid-turn while it waits, so nobody answering
> is itself an answer. That is why it carries the identity of the request — an answer is for one
> tool call — and **how long there is to give one, because "approve this" with no clock is a
> promise this plane cannot keep.**

The variant has three fields: `tool`, `detail`, `tool_use_id`. There is no clock. What reached the
phone was `wants to use Bash: rm -rf /important — it is waiting for an answer`. The patience was in
scope at the `LogKind::Asked` call site the entire time and went to `tracing::info!` — on the one
machine nobody is logged into. `Notice::Overdue`, the arm directly above it, carries a duration and
renders it.

This matters beyond tidiness because `offload approve` addresses an answer to the agent's own
`tool_use_id`, which exists only while that process is blocked. Somebody who picks the phone up
twenty minutes later approves a call that no longer exists, and nothing had told them there was a
window at all.

### 2. Nothing closed the loop, and the reason given was a claim about a message this default discards

`notable` excluded the whole `Answered` kind:

> **`Answered`**, for `Cancelled`'s reason: whoever approved it knows. The one answer nobody gave
> is a *denial*, and what a run did without permission is **what its result reports**.

The first clause is right for `Allowed` and `Denied` — a person acted and knows. It is not right for
`Unanswered`, and the fallback offered in its place is a claim about the *result being delivered*.
ADR-0026's `Notices::Problems` discards exactly the result (`is_good_news` is `Notice::Finished`
and nothing else), and `Problems` is a **rule's** default — which makes a rule precisely the run
that asks with nobody there and is never told what happened.

So the measured sequence, for each firing: the phone says *"run needs a decision: wants to use Bash:
rm -rf /important"*; five minutes pass; the wait runs out; the agent applies its own rules; the run
completes; and nothing is ever said again. The question also stops existing in `offload asks` at
that moment — correctly, since a pending question lives exactly as long as the blocked process — so
neither the phone nor the CLI has it, and the only record is `LogKind::Answered` in the run's own
log, which somebody would have to know to go and read.

`Notices::Problems`' own doc comment had already stated the principle:

> `overdue` and `asked` are problems too — **an unanswered question blocks the run mid-turn until
> its patience runs out**, which is the last thing to be quiet about.

Half of that was built. The question got through; the fact that its window closed did not.

## Decision

### 1. The question carries how long there is

`LogKind::Asked` gains `within_ms`, populated from the `patience` already computed one line above
it, and `Notice::NeedsDecision` gains `within: Millis`. A **duration**, like `Overdue`'s `by`, for
this crate's usual reason: `offload-core` has no clock, and the reader of a notification wants "you
have five minutes" rather than a timestamp to subtract. `offload logs` prints it too.

### 2. `Answer::Unanswered` is news; `Allowed` and `Denied` stay silent

`Notice::Undecided { tool, why }`, projected from `LogKind::Answered { answer: Unanswered, .. }`
only. The carve-out that was always right keeps its reason — whoever acted knows — and the one
answer nobody gave now closes the item it opened.

`why` is the log's own `by` string, which is already the sentence: *"nobody answered within 5m0s"*
or *"the agent stopped waiting"*. Two different facts, and a person can act on the difference: the
first is a window they missed, the second is an agent whose hook timeout is shorter than this
node's patience.

`tool` is denormalised onto `LogKind::Answered` because the projection sees one event at a time,
and a notification that names a `tool_use_id` at somebody holding a phone says nothing.

### 3. `NOTABLE_KINDS` gains `"answered"`, and that is a scan bound rather than a decision

The same structure ADR-0026's escape hatch already had to be fixed for: `answered` names **three**
outcomes of which one is news, exactly as `finished` names two. The list bounds the scan; the
projection decides. Filtering on the name would be the identical bug one variant over.

### 4. `Problems` needed no change

Any notice that is not `Finished` gets through, because `is_good_news` is a predicate over one
variant rather than a list to extend. That is the shape ADR-0026 settled on after the name-based
version failed, and it is why a new notice is admitted without anything being enumerated anywhere.

## Consequences

- A person interrupted by a run is told how long they have and told when the window shut. Measured
  after the change, on a rule with the default `--notify-on problems`:
  `wants to use Bash: rm -rf /important — answer within 59.9s`, then `nobody answered within
  59.9s, so Bash was left to the agent's own rules`.
- Two extra notifications per asking occurrence, worst case. That is the honest cost of the feature
  and it is bounded by patience rather than by the firing rate — see below.
- No wire or schema change. `LogKind` is serialised into the event log and both new fields are
  `#[serde(default)]`, so rows written before this build decode with `within_ms: 0` and an empty
  `tool`. A pre-existing `Asked` row therefore renders "answer within 0ms", which is wrong and
  harmless: the question it describes stopped existing when its process did, so nobody can act on
  it either way.

## What this deliberately does not change

**A blocked occurrence throttles its own rule, and that is ADR-0020 §3 working.** Measured: 122
dropped events across 2 firings, because each blocked occurrence holds the rule for the whole of
its patience. A three-second watcher becomes a five-minute watcher the moment it starts asking.
That is the *stated* behaviour — one occurrence at a time, an event arriving while the last is
still going is dropped and counted, because a watcher's value is the current state — and the
alternative is queueing, which §3 refuses for reasons that have not changed. It is visible where it
should be: `offload rules` prints `fired`, `dropped` and `kept`.

**`--ask=N` is per occurrence, not per rule.** `RunProgress::asks` is per run and a firing is a
run, so a rule's total questions are bounded by the firing rate rather than by `N`. Left alone
deliberately: the budget's job is to stop *one* run putting thirty questions on a phone, patience
already bounds the rate at one question per occurrence-wait, and a per-rule budget would be a
counter on a node-local record that has to survive a prune (ADR-0021) — machinery for a bound
patience already supplies. Reconsider if somebody sets a long `--deadline` on a fast rule, which is
what would make patience stop being the limiting factor.
