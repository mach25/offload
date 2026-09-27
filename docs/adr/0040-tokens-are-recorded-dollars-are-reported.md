# ADR-0040: Tokens are recorded from the transcript; dollars are only ever the agent's

**Status:** accepted · 2026-08-29 · amends nothing · supersedes nothing · **built** (amended below)

## Context

ADR-0039's residual: a run stopped by its turn limit reports no cost. `cost_micro_usd` is set
only from the agent's final `result` event, and a stopped agent never emits one — so `offload ps`
prints `-` in the column the operator capped the run to watch. Wider than the turn limit: a
released checkpoint, a drain and a cancel all end without a result too.

Session thirty-one wrote the fix into the roadmap as "per-turn accounting from each assistant
message's `usage`". **That was reasoned, not measured, and it is wrong.** Measured here against
claude 2.1.251 on two legs of one real conversation:

1. **The live stream's assistant `usage` is a partial snapshot.** The three assistant messages
   reported `output_tokens` of **1, 2 and 4** on the wire; their true finals were **236, 85 and
   42**. `stop_reason` is `null` on every streamed copy. Summing the stream undercounts by about
   fifty times.
2. **One assistant message is emitted once per content block**, each copy carrying the same
   `message.id` and the same `usage` object. A thinking block and a tool call are two events and
   one message. Summing rows rather than messages double-counts exactly: **726 against a true
   363**.
3. **The transcript carries the final usage, and it is cumulative across legs.** Deduplicated by
   `message.id`, the transcript reconciles **exactly** with the sum of every leg's `result` event,
   on all four counters — input 44, output 517, cache-creation 9,474, cache-read 104,639 — across a
   fresh leg and a `--resume`.
4. **Each `result` event's `total_cost_usd` is that leg's alone**, not the conversation's:
   $0.026998 for the fresh leg, then $0.006026 for the resumed one. This is load-bearing and was
   until now assumed: `Supervisor` does `cost_micro_usd.saturating_add(cost)` once per leg, which
   is **correct precisely because of this**, and would silently double every migrated run's cost if
   the agent ever made that field cumulative.

The result event also reports `costBasis: "list"` beside its `modelUsage` — the agent is itself
converting tokens to dollars at list price, and says so.

## Decision

1. **Tokens are recorded; dollars are never computed.** Token counts are a fact the agent
   *states*. Dollars are a derivation from a price list, and shipping our own would put a third
   number in the system that disagrees with the other two — stale the week a price changes, wrong
   in a way nobody notices, and wrong in principle anyway, since `costBasis: "list"` is not what a
   given account is billed. "Don't over-claim" applies to a report exactly as it applies to a
   probe.

2. **The token source is the transcript, deduplicated by `message.id`** — never the live stream
   (§1) and never the raw rows (§2). Offload already reads the transcript at every checkpoint, so
   this costs one parse of bytes that are in hand, and it lands the numbers at the moment a
   capped, checkpointed, drained or cancelled run most needs them. **See the amendment: one read
   is not enough.**

3. **The two numbers have opposite merge rules, and that is the whole of the care needed here.**
   A `result`'s cost is **per leg and adds**; a transcript total is **cumulative and replaces**.
   Mixing them — adding a cumulative total, or replacing with a per-leg one — double-counts or
   erases. This is ADR-0005's one-field-two-facts shape again, and it is why tokens get their own
   field with its own rule rather than being folded in beside cost.

4. **Owner, per the standing rule for a gossiped field: the run's holder, forward-only within a
   leg, and settled between legs exactly as `RunProgress` position already is** (ADR-0005's second
   amendment). Tokens are a position, not a spend: they are re-derivable from the transcript, and
   the holder that has the transcript is the only node that can state them.

5. **A dollar figure stays absent rather than estimated.** A run that never produced a `result`
   has no cost and says so — `-`, which is what `ps` already prints. Absent is the honest answer,
   and the tokens beside it are what makes it a useful one.

**Rejected: a per-model price table.** It is the obvious build and it is a maintenance liability
with a silent failure mode. Four models today, four rates each (input, output, cache write, cache
read), plus the multipliers, plus per-account plans that make list price the wrong basis. The day
one number goes stale, every report in the system is confidently wrong and nothing says so.

**Rejected: summing the live stream.** §1 and §2. It is the version somebody will reach for
because the events are already flowing past.

## Consequences

* Unbuilt, deliberately, and specified so that building it is mechanical: a pure
  `offload-agent::usage` module over transcript bytes; token counters on `RunProgress` with the
  merge rule in §3/§4; a schema migration and a wire bump; a column in `ps`.
* **`Supervisor`'s per-leg `saturating_add` of cost is verified correct** (§4) rather than
  assumed, which is the one thing here that was about shipped code. No change follows from it.
* The residual it does *not* close: a stopped run still has no dollar figure, by decision rather
  than by omission. What it gains is a number that means something beside the blank.


---

## Amendment, the same day: the transcript lags the stream, so it is read twice

Building §2 as written produced a `TOKENS` column that was blank for exactly the run it exists
for. Walked with a real agent: a run capped at one turn reported `-`.

**The agent writes its transcript behind its event stream, by an amount it does not promise.** The
checkpoint taken at the turn-1 boundary captured **16,999 bytes** of transcript containing
`queue-operation`, `user`, `attachment`, `atis-latch` and `ai-title` rows and **no assistant rows
at all**; the same file on disk after the run held three. The parse was correct and the data was
not there yet. A per-checkpoint read is therefore always about a turn stale and can be empty
outright — and it is emptiest for short runs, which is what a cap produces.

So the read happens **twice**: once inside `checkpoint`, on bytes already in hand, which keeps a
long run's number moving while somebody watches it; and once when the leg ends and the agent's
process is gone, at which point the file is final. The second read lives inside the `describes`
branch rather than beside it, because it asks that branch's question — these numbers gossip, and a
leg that has lost the run may not state them. Neither read overwrites a good number with an empty
parse. Re-walked: a run capped at one turn reports **21.8k**, matching the transcript's own totals
(10 + 217 + 3,740 + 17,838 = 21,805) exactly.

**And the walk it pointed at, done: the answer was worse than the guess.** The same lag means a
checkpoint taken at an early boundary stores a transcript missing the most recent turns, and at
turn 1 one with no assistant messages at all. The guess written here was that the agent would redo
a turn. It does not. Resumed onto a rebuilt worktree — the migration path, with `transcript
restored from checkpoint` in the log and the stored blob written over the agent's own current file
— the agent replied **"No response requested."**, made no tool call, wrote nothing, and the run
reported **`Completed`**: an abandoned run that reads as a success. The control, whose capture had
caught up, resumed and wrote the nanosecond timestamp that had existed only in the conversation.

The fix is not this ADR's to make and needs no ADR of its own, because `checkpoint` already stated
the rule — *better to fail the checkpoint loudly than to record one that cannot do its job* — and
was only asking the weaker question of whether the file existed. It now refuses a capture whose
transcript has no assistant messages in it. Two things fell out of walking that: **being about a
turn behind is the normal state** (measured at every boundary of a healthy five-turn run), so only
*nothing* is fatal and a warning on the lag was removed for firing on every run; and **a refused
capture consumed the operator's checkpoint request**, which is its own entry in
`docs/pitfalls/lifecycle-and-recovery.md`.
