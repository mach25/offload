# ADR-0035: A drain reports while it drains, because the thing it waits for is often somebody's to release

**Status:** accepted · 2026-08-27 · builds ADR-0034's stated residual · corrects ADR-0034's
placement of the departure flag · supersedes nothing

## Context

ADR-0034 left one finding unfixed on purpose and named the two candidate fixes. Walked, it is
worse than it reads there, and the walk found two more things about the same command — one of
them introduced by ADR-0034 itself.

The harness is the cheap one: a fake agent that poses a *real* permission question by piping a
`PreToolUse` payload into `offloadd ask-hook`, a `[[sinks]]` route that appends to a file, and one
daemon with a mesh. No model spend.

### 1. `offload drain` printed nothing at all, for as long as it took

```
17:53:13
exit=124 at 17:53:53          ← `timeout 40 offload drain`; nothing above this line is its output
```

Forty seconds of a blank terminal, and it would have been five minutes: `DEFAULT_ASK_PATIENCE` and
`drain_deadline_secs` are both 300 seconds, and a run blocked mid-tool-call reaches no turn
boundary until one of them expires. Meanwhile, in the next window:

```
$ offload asks
01a043eca9f3   Bash   1m3s   3m56s   here   rm -rf /tmp/nothing
                 offload approve 01a043eca9f3 toolu_walk_1

$ offload status | grep waiting
waiting     1 run(s) stopped for an answer — `offload asks`
```

Two other commands could say exactly what the drain was waiting behind, and the drain — the
command somebody is standing in front of with their hand on the lid — could not. The wait was
*avoidable*: answering it took one line and released the drain in the same second.

### 2. …and it reported the good outcome as the worst one

Answering revealed a second fault. `wait_for_checkpoint` returned `Option<Run>` and answered `None`
for **three different reasons** — deadline reached, record gone, and *the run finished by itself*,
which its own comment called "the nicest possible outcome". The caller read all three as one:

```
WARN still mid-turn at the drain deadline; leaving it to the lease   ← 150ms after `finished`
INFO handed over what the fleet would take  moved=0 left=1
```

`left`'s doc comment says "mid-turn at the deadline, or refused by every peer", and the CLI prints
*"a run still here was either mid-turn at the deadline or refused by every other node"*. About a
run that had completed. That sentence is what keeps a laptop open for nothing, which makes it the
same family as ADR-0025's `offload logs` fallback: **when a value is `None` for more than one
reason, a single answer is right for at most one of them.**

### 3. …and ADR-0034's own fix stopped the *signal* path stopping accepting

ADR-0034 moved `supervisor.stop_accepting()` out of `Mesh::drain` and into `Request::Drain`,
reasoning — correctly — that "this node is leaving" is a fact about the node rather than about the
handover pass, and — also correctly — that it should have **one owner** rather than a check at
every call site. It then made it a call-site check at one of that function's two call sites. The
other is the `SIGTERM` shutdown, which is how a laptop actually leaves: a service manager stopping
the daemon, a logout, `pkill offloadd`.

Measured, from the daemon's own log:

```
15:55:41  SIGTERM
15:55:41  draining runs=1
15:55:44  spawning claude code  run_id=01a043ef3018…     ← three seconds into the shutdown
```

with, at the keyboard:

```
$ offload status | grep accepting
accepting   yes
$ offload run --repo … -- "work arriving during shutdown"
run 01a043ef3018            ← accepted, and started
```

A node in the act of leaving took new work, spawned an agent for it, and was then killed by its own
shutdown. This is the fourth time this flag has been in the wrong place, and the second time the
*same sentence* has described what was left behind.

## Decision

### 1. The departure is one fact with two ways to be told, so it gets a function

`mesh::depart(supervisor, mesh: Option<&Mesh>, deadline, window, report)` sets the flag, says so,
and then runs the handover pass if there is a fleet to run it against. `Mesh::drain` is **private**,
so `depart` is the only way in and the compiler is what keeps the two callers in step — a rule
enforced by nothing is what produced (3).

`mesh` is an `Option` for the reason the flag was missed the *first* time: a fleet of one is a
departure too, and stopping accepting is the whole of what a drain can do there (ADR-0034 §1).

### 2. `offload drain` streams

`Response::Draining(DrainStep)` before `Response::Drained`, over the control socket's existing
one-request-many-responses shape — the same mechanism `logs --follow` uses, and no wire bump,
because this protocol is unversioned and both binaries ship together. The CLI moves from
`client::request` to `client::stream`.

Three steps, and not a running log of everything:

* **`StoppedAccepting`**, the moment it is true. It is the one thing a drain always does, the only
  thing it can do on a fleet of one, and it was being printed at the end of a pass that can take
  five minutes.
* **`Waiting { runs, up_to_ms }`**, once — the checkpoints are all requested before any of them is
  waited on, so what an operator is in is one wait bounded by one deadline.
* **`Blocked { run, tool, detail, tool_use_id, within_ms }`**, which is the reason any of this
  exists. It carries the `tool_use_id` because `offload approve` addresses one tool call: without
  it the line is information, and with it it is the way out.

The steps go through `mesh::Report`, which is `silent()` for the shutdown path — there is no client
there — and which sends with `try_send`, because a client that has stopped reading must not be able
to hold up a departure. The blocked step is *also* logged, since "why did stopping this daemon take
five minutes" is the same question on the path with nobody to tell.

### 3. Four outcomes for a wait, because there are four

`Boundary::{Reached, Finished, StillMidTurn, Gone}`, and `Drained` gains `finished` beside `moved`
and `left`. A run that ended by itself is neither handed over nor left behind, and it gets its own
sentence: `nothing to hand over — 1 run(s) finished while it waited`, with none of the "either
mid-turn or refused by every other node" advice attached. `Gone` — the record vanished under us —
counts in `left` and logs its own reason, because unknown is not good news.

### 4. Reporting, not expiring

ADR-0034's other candidate was for the drain to **expire** the pending questions of the runs it is
draining, on the grounds that they are unanswerable once the node is leaving. Rejected: they are
not unanswerable, they are *unanswered*, and the walk shows the difference — the answer arrived
seven seconds after the line asking for it, and the run then completed normally. Expiring the
question would decide somebody's tool call on their behalf in order to save the drain a wait, which
is the same trade as snapshotting mid-turn.

## Consequences

- Measured after the change, four seconds into the drain, where there used to be nothing:

  ```
  this node has stopped accepting work — restart offloadd to take work again
    waiting for 1 run(s) to reach a turn boundary — up to 5m0s
    run 01a0440bbca7 is stopped waiting for an answer: Bash — rm -rf /tmp/nothing
      it reaches no turn boundary until that is decided, or 4m59s from now
      answer it: offload approve 01a0440bbca7 toolu_walk_1   (or `offload deny`)
  ```

  and after answering it as the line says to, in the same second:

  ```
    nothing to hand over — 1 run(s) finished while it waited
  [the command returned at 18:27:08]
  ```

- The `SIGTERM` path: `accepting no — drained` three seconds after the signal, and a submission
  arriving then is refused with `alpha draining`. It still *submits* — a laptop being closed may
  ask the desktop to work, which is ADR-0034 §2's deliberate distinction — and the shutdown log now
  carries the blocked-question line and `moved=0 left=0 finished=1`, where it used to say `left=1`
  about a run that had finished.
- `offload drain` on an idle node and on a fleet of one read exactly as ADR-0034 left them.
- No schema change and no wire change. `Drained::finished` is `#[serde(default)]`, which matters
  only for a CLI and daemon of different builds sharing a socket.
- The three guards are in `mesh.rs`, and each was watched red: the departure that happens with
  nothing to hand over, the four ways a wait ends, and the blocked question being said once. The
  third hung rather than failing on the first attempt — `recv().await` with a live sender waits for
  ever — so the helper takes a timeout, because a guard that hangs instead of going red is not a
  guard.

## What this deliberately leaves

**The drain's deadline does not bound the question's patience.** Both are 300 seconds, so which
expires first depends on when the question was asked relative to the drain — a question raised
*after* the drain began outlives it, and the run is then left mid-turn even though it would have
reached a boundary moments later. Shortening the patience to the drain's remaining time is the
weaker form of the expiry rejected in §4 and has the same objection; the alternative is a longer
drain deadline, which is a number to argue about rather than a mechanism. Left as it is, now that
the wait says what it is waiting for.

**Amended 2026-09-05 (session sixty-one), and the residual was half wrong about itself.** The
mechanism stays exactly as decided — nothing expires a question to save the drain a wait, because
they are unanswered rather than unanswerable. What the residual did not notice is that §2 was
*already broken by it*: `Blocked` carried `within_ms` from the question's own patience and knew
nothing about the drain's deadline, so this section's own case — the question outliving the drain —
printed a horizon that was true about the question and wrong about what the operator was waiting
for. Walked on one daemon with `drain_deadline_secs = 15`:

```
  waiting for 1 run(s) to reach a turn boundary — up to 15.0s
  run 01a07310d9b4 is stopped waiting for an answer: Bash — rm -rf /tmp/nothing
    it reaches no turn boundary until that is decided, or 4m52s from now
```

Two lines apart, and the second is the one somebody acts on. `Blocked` now carries
`drain_ends_in_ms` beside `within_ms` and the CLI names whichever is nearer, with the consequence,
because they are *different events*: the question expiring decides it and the run is handed over
normally, while the drain expiring first decides nothing and leaves the run mid-turn. Measured
after, same staging:

```
    it reaches no turn boundary until that is decided — and this drain stops waiting in 15.0s,
    which is sooner
    unanswered by then, the run is left mid-turn here; the question itself stands for 4m52s
```

This is §2 finished rather than §4 reopened — reporting, not expiring. The residual above stands
for the mechanism and is closed for the report.

**A run blocked on a question is still not migratable mid-turn**, and nothing here changes that
(ADR-0004). What changed is that the person who can end the wait is told, in the command they typed.
