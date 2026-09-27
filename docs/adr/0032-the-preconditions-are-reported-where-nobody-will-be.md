# ADR-0032: The delivery plane's preconditions are answered at the keyboard for a run, and were answered nowhere for a rule

**Status:** accepted · 2026-08-27 · completes ADR-0010's audience note and ADR-0017's ask note ·
supersedes nothing

## Context

Two flags make a promise the fleet may not be able to keep, and both are answered while the
operator is still at the keyboard — ADR-0014's rule, applied to the delivery plane:

- `--notify <service>` asks for a route. `deliver::audience_note` says so when there is no such
  route, or when the only one is broken.
- `--ask` changes what the *run does when it is blocked*. `deliver::ask_note` says so when nothing
  in the fleet can reach a person, because the flag then amounts to the behaviour of not having
  passed it.

Both notes ride on `Response::Submitted`. `Response::Watching` carried neither, and `offload when`
accepts both flags. So the two commands, one after the other, on one fleet with no routes:

```
$ offload run  --repo … --ask --notify push -- "check the build"
run 01a0437323b6
  no push route in this fleet, so nothing will tell you — `offload sinks` lists what there is
  nothing in this fleet can reach a person, so --ask will stop nothing — its questions are
  decided by the agent's own rules, as they are without it

$ offload when nightly --repo … --ask --notify push -- "check the build"
rule 058e26b9c47903b9 — on `nightly`
  Quiet while it works: you will hear about a failure, a missed deadline or a question, and
  not about a firing that went fine. `--notify-on everything` for a heartbeat.
```

The warnings are on the wrong command. A run is answered for by somebody who is sitting in front
of it; a rule is written once and then fires unattended for months, which is the whole of what
ADR-0020 is for — so it is the command whose promise nobody will be present to check, and it was
the one that said nothing.

And in place of the two warnings it printed a **reassurance**, naming three notifications — a
failure, a missed deadline, a question — of which that fleet could deliver none. With `--ask`
unreachable there would never even *be* a question.

Three more measured cases, from the same walk:

1. **The plainest possible invocation.** `offload when nightly -- "check the build"`, no flags,
   on a fleet with no routes: the promise is printed. This is the case `audience_note` cannot
   cover, and the reason is written in its own doc comment — it returns `None` for
   `Audience::Everyone`, "which needs no words". True in front of a keyboard. False for a watcher.
2. **`--notify nobody --notify-on everything`** printed `Every firing will be reported`. Wrong on
   *any* fleet, routes or none: ADR-0026 made these two independent axes, and the sentence about
   *kinds* was printed without consulting the axis that zeroes it out. `Audience::Nobody` is no
   route, so no kind reaches anybody.
3. `offload run --notify nobody` prints `nobody will be told when it finishes — you asked for
   none` correctly, one command away.

The `offload when` handler already carried the reasoning, in a comment above the sentence that was
wrong: *"A default that differs from `offload run`'s has to be said, once, where it is chosen. A
rule is written and then fires unattended for months."* It was applied to `--notify-on` and to
neither of the two flags beside it.

## Decision

### 1. `Response::Watching` carries the same two notes as `Response::Submitted`

`audience` and `ask`, from `audience_note` and `ask_note` — the *same functions*, called with the
same inputs, so a rule and a run cannot say different things about one fleet. That is the rule
`can_reach_a_person` already exists to enforce between the submission and the blocked agent,
extended to the third caller.

No wire bump: the control protocol between `offload` and `offloadd` is deliberately unversioned
(`api.rs`: "local, same-user"), and both binaries ship together.

### 2. `deliver::Reach` — where a rule's news can get to, as three answers and not a bool

```rust
pub enum Reach { Somebody, NobodyWanted, NoRoute }
```

A bool would collapse two of them. A silence the author **asked** for is not a silence the fleet
imposed: printing "add a route" under `--notify nobody` is advice about a setting somebody chose on
purpose. Same shape as `Removal::{Removed, NothingHere}`, `Met::{Handshakes, NotAsked}` and
`Prune::{AtAFiring, Finally}` — one word for two facts is how a report comes to answer the wrong
one.

It is computed through `Audience::admits`, asked **per route** over the same local-and-peer route
set the delivery pass walks, because that is what the pass asks (ADR-0010: an audience selects
routes, so the question is asked per capability). A second copy of the matching rule here is how a
report comes to disagree with the plane it describes. A configured route counts before it is known
to work, for `can_reach_a_person`'s reason: broken or asleep is a route, and `audience_note` is
what says it cannot be used today.

### 3. It is asked by `offload when` and not by `offload run`

The two differ in exactly one place and it is the **default**. `Audience::Everyone` on a fleet with
no sinks is "needs no words" in front of somebody at a keyboard — they can see the run, and
`offload sinks` explains the fleet. For a rule it is a watcher that will fire for months with
nothing able to carry what it says, and it is the invocation everybody types first.

### 4. The kinds sentence is a promise only where the fleet can keep it

The `--notify-on` line still says which way the switch went — it travels with every occurrence,
and this fleet may gain a route tomorrow — but in the future tense only under `Reach::Somebody`:

| reach | what is printed |
|---|---|
| `Somebody` | *"you will hear about a failure, a missed deadline or a question…"* (unchanged) |
| `NobodyWanted` | *"Set to report problems — kept on the rule, and carried to nobody while the audience is none."* |
| `NoRoute` | *"Set to report problems, and there is nothing in this fleet to report it to, so the rule will fire silently — including when it fails."* |

`including when it fails` is the clause that matters: a watcher that fires and succeeds is already
silent by design (ADR-0026 §3), so "no notification" and "no failure" look identical, and the one
thing a watcher exists to say is the one a missing route eats.

## Consequences

- Both flags now say what they will amount to, on the command where nobody will be present to
  find out otherwise. Measured on one node, five audiences, before and after.
- `Reach` is a report, never a gate. A rule with nowhere to send its news is still written and
  still fires — the work is worth doing, and ADR-0010's plane is a second plane precisely so that
  a delivery failure is not a run failure. Same rule as `audience_note` being a note and never a
  refusal.
- It is a snapshot, like every other answer of this kind here. A fleet that loses its last route
  the day after a rule is written gets no second warning, and could not: the fix would be a
  notification about the notification plane, delivered through the plane that is missing.
  `offload sinks` and `offload rules` are where that is visible, and `offload status` prints the
  route count.
- Nothing about `Origin` reaches this. ADR-0024 forbids it and the prohibition is right here as
  well: what is being reported is a property of the *submission's flags and this fleet's routes*,
  and an operator's run typed with `--notify push` on a fleet with no push route deserves the same
  sentence — which it has had since session nine.

## What this does not reach

A rule placed on a **peer** is reported against the routes visible from the node the rule was
written on, which is the right answer — the delivery plane fans out from wherever the news is
noticed, and a route on any node in the fleet counts. But it is answered from *this* node's view,
which is a gossip tick stale, and a peer whose sink capability has not yet been learned reads as
`NoRoute`. Over-warning, in the direction of the printed advice being harmless, and it settles
within one tick.
