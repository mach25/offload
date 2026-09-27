# ADR-0010: Reporting is a capability, and delivery is a plane of its own

**Status:** accepted · 2026-07-26 · built 2026-08-20 (forward direction and the audience; the reply path remains)

## Context

Today a run's output goes to exactly one place: the event log in its holder's store, read
back over that node's control socket by `offload logs`. That works because there is one
node and the operator is sitting at it. Two things break it, and both are close:

- **Runs move.** After phase 4 the events for one run are spread across every node that
  hosted it, and "tail the log on the machine doing the work" stops being a sentence with a
  single answer.
- **The person is not at the machine.** The premise of the project is that work happens on
  whatever device is best placed for it — which is precisely the device the human is *not*
  looking at. A run that finishes on the desktop while its owner is out with a phone has
  nowhere to say so.

There is a matching gap pointing the other way. ADR-0008 refuses `PermissionMode::Ask`
because a headless run has nobody to answer a prompt. That is the same missing thing: a
road between a run and a human who is somewhere else.

And it is not only agent runs. A device that cannot host an agent at all — a phone that can
poll an HTTP endpoint on a schedule — is still useful, and the *entire point* of that work
is to report something back. For that workload, delivery is not a side channel; it is the
deliverable.

## Decision

**Two planes, named and kept apart.**

- The **execution plane** answers *who does the work*: nodes, capabilities, bids, leases,
  epochs, checkpoints. Everything built so far.
- The **delivery plane** answers *who hears about it*: which humans, through which devices,
  by which route.

A node participates in either, both, or — importantly — only the second. A phone with no
agent and no toolchains is a full member of the fleet if it can deliver a notification.

**Reporting routes are capabilities.** This is the load-bearing decision, and it is what
keeps the delivery plane from becoming a parallel universe with its own scheduler. A sink —
`push`, `email`, `chat`, `webhook`, an attached `terminal` — is a fact about a device in
exactly the way `agent=claude-code` is:

- It is **ability, not permission.** "This device holds a push token for its owner" is a
  capability; "not while I'm asleep" or "not on mobile data" is `WorkPolicy`. The existing
  split already carries the distinction and the refusals stay legible either way.
- It is **probed, and never over-claimed.** A sink with no working credential is
  `authenticated: false`, the same rule the agent probe follows. A node that claims a
  delivery route it does not have wins the routing decision and then silently drops the
  message, which is worse than not offering it — silence is the one failure mode this
  plane cannot detect on its own.
- It is **matched by `Constraint`.** Routing a notification is choosing a node that
  satisfies `sink=push AND authenticated`, under policy, with the same scoring and the same
  "why didn't that happen" explanation the placement path already owes its users.
- Its **credentials do not migrate.** Same rule as agent auth (ADR-0002): a push token
  belongs to the device that enrolled it. If a design requires shipping one to a peer,
  stop.

**The event log stays the source of truth; delivery is fan-out from it.** Notifications are
never a side effect buried in an agent adapter or a run. They are derived from the durable,
sequenced event log the store already keeps, which is what makes them survivable: a run's
notification is not lost because the node that would have sent it went away mid-turn.

**A `Notification` is a small typed subset of run events**, not the stream. Run finished,
run failed, run needs a decision, watcher condition met. Deciding what deserves a human's
attention at the source keeps every sink dumb and interchangeable, and keeps the volume
survivable — nobody wants a push per tool call.

**Delivery is at-least-once, deduplicated on `(run, seq, sink)`.** Runs migrate and nodes
die; a cursor per sink lives in the store. A notification arriving twice is a nuisance, and
one that never arrives is the failure the plane exists to prevent. That asymmetry decides
the trade.

**Replies are fenced inputs to the run.** An answer to a permission prompt comes back
addressed by `(run, epoch)` and is rejected if the run has moved on, exactly like every
other side-effecting path. An approval granted for a turn that no longer exists must not
apply to whatever the run is doing now.

Nothing here is built yet. What it forbids, starting today: no code may assume the operator
is attached to the node doing the work, and no notification may be emitted as a side effect
from inside an adapter.

## Consequences

Good, and mostly by reuse:

- The mesh never learns what email is. Sinks are adapters behind one `deliver` call, the
  same shape `offload-agent` gives the execution plane.
- `offload logs -f` becomes one sink among many rather than the only way to see anything.
- A watcher run's "tell me when this changes" and an agent run's "may I run this command"
  travel the same road, so the hard part is solved once.
- A device too small to work is not too small to join. That is a real change in what the
  fleet is for, and it costs nothing structurally because capability already gates by fact
  rather than by device class.

Bad, and worth being clear-eyed about:

- **Egress is a grant.** A node that can deliver a webhook can make outbound requests to an
  address something else chose. The tool-allowlist reasoning transfers almost unchanged,
  including its sharpest lesson: a pattern that looks scoped and is not is worse than no
  allowlist, because it is trusted.
- **Duplicates are visible to humans.** At-least-once means a failover can double-notify.
  Acceptable, and cheaper than the alternative, but it will be noticed.
- **Deciding what is worth interrupting someone for is a judgment call**, and the first set
  of `Notification` variants will be wrong. Keeping the set small and typed makes it cheap
  to be wrong.
- **A sink can be slow or unavailable in ways a run cannot.** Delivery must never block a
  turn boundary or a checkpoint; a notification that cannot be sent is retried from the
  log, not waited on.

## Amendment, 2026-08-20: what building the local half settled

Notifications now leave a node. Six things the ADR left open, decided by building it.

**The first sink is a command the owner nominated, and that is not a compromise.** There is no
push credential, no mail transport and no HTTP client in this tree, and a route that cannot be
verified must not be claimed — so the honest first sink is the one where the owner writes down
what reaches them. What two lines of shell can reach is everything: `notify-send`, `ntfy`,
`mail`, a webhook via `curl`. It also puts the ADR's own egress warning where it belongs: the
exact command is the owner's, not a pattern language this project invented and would get wrong.

**The service is declared; the transport is nominated.** A sink says `service = "push"` *and*
`command = "…"`, which looked redundant and is the thing that makes routing possible later. Had
every exec sink been one service — `Service::Command`, say — then "reach me by push" would be
inexpressible and every route on every device would be indistinguishable. Instead the existing
`Constraint::HasService` already asks the right question, and the only claim the machine makes
for itself is `authenticated`, which is exactly what it can check: **the program exists**. It
cannot check that the script reaches a human, which is the same limit an email sink with a
working SMTP server and a dead mailbox has.

**Fan-out is an outbox, and it is two mechanisms on purpose.** A cursor per sink bounds the
scan; a row per `(sink, seq)` is the dedup identity and carries the outcome. Collapsing them —
advancing a cursor only on success — makes a broken sink swallow everything behind it, which is
the failure mode this plane exists to prevent, arriving through the door marked "simpler". The
primary key is what makes it at-least-once: the insert that loses a race is a no-op, so *this*
node never double-sends, and two nodes both delivering is the duplicate the ADR already accepts
by name.

**A new sink starts at the end of the log, not the beginning.** Not stated above and the plane
is unusable without it: a cursor initialised to zero hands a route configured this evening every
notable event the node has ever logged. A month of finished runs arriving at once is how somebody
learns to turn notifications off.

**"Not waiting any more" is not "arrived".** The first `offload sinks` counted abandoned
notifications as delivered, because both are resolved rows — so a route that had never worked
once reported four successful deliveries. They are counted apart now, and a route that is usable
*today* says out loud how many it dropped earlier. A delivery plane that cannot explain a silence
has failed at its only job, and one that reports a healthy number while doing so is worse than
silent.

**And the bug that came out of it, which is not about delivery at all.** A failed *capture* was
being logged as `LogKind::Failed` — the *run's* terminal state — and everything downstream
believed it: `LogEvent::is_terminal` said so, which hung up every follower mid-run and with them
the attendance that decides whether a failure resumes itself (ADR-0013); and the new projection
dutifully told somebody their run had failed, a minute before telling them it had finished. It is
`CaptureFailed` now. The general rule is worth more than the fix: **a log kind named after a state
must not be borrowed for an operation that happens inside it**, because a log is read by things
that cannot ask what was meant.

## Amendment, 2026-08-20: routing to a peer, and the promise that nearly broke

The demo this ADR was written for now runs: a run finishes on a machine with no route to a human,
and a device that hosts nothing — enrolled with `{Submit, Deliver}` and never granted
`host-runs` — is the thing that tells somebody. Four things it settled.

**The sender keeps the outbox; the peer answers and forgets.** The node that logged the event is
the only node that knows what it has already said, so it is the only node that can be
at-least-once about it. A peer receives an ask, runs its route, and replies — no state, no
retries, nothing to reconcile. Being asked twice is the contract working.

**Nothing about a route travels.** A peer is told *what* to say and never *how* it says it: the
message carries a notification and the id of a capability that device advertises, and that is
all. The command behind it, its arguments and whatever credential it uses stay where they are.
This is ADR-0002's rule for agent auth, applied to the other plane, and it means the routing
decision is made from gossiped capabilities alone.

**`Deliverer` is a separate registration from `Host`, and that is the whole claim of this ADR in
code.** It would have been cheaper to hang `deliver` off the trait the cluster already uses to
reach the daemon — that trait has picked up non-hosting methods before. But the interesting
device answers `NoHost` to every run and implements this one, and a plane that is "kept apart" in
the prose while sharing an interface in the code is apart only until somebody is in a hurry.

**Away is not gone, and conflating them broke the promise.** The first version treated a route it
could not reach as a route that no longer existed: the queued notification was retried three
times over fifteen seconds and abandoned. Which is precisely backwards — the whole promise is
*you will be told when you pick your phone up*, and a phone is asleep for hours. So a route on a
device the fleet still knows is **listed, queued for, and waited on indefinitely**, with nothing
attempted and nothing spent from its retry budget while the device is away; a route on a device
the fleet no longer lists at all is given up on, because there is nothing left to wait for. The
distinction is *known* versus *answering*, which is the same distinction ADR-0007 makes about
holders and for the same reason. `offload sinks` says which: "phone is not answering — 2 waiting
for it to come back".

The cost is stated rather than hidden: rows accumulate for a device that never returns. They are
tens of bytes, and the answer for a device that is gone for good is to revoke it.

What is deliberately still missing:

- **The reply path**, so `PermissionMode::Ask` becomes answerable (ADR-0008). The forward
  direction existing changes nothing about the fencing that half needs.
- **Terminal as a sink.** `offload logs -f` is still its own thing rather than one sink among
  many. Nothing is gained by converting it until there is a second real sink to be uniform with.

## Amendment, 2026-08-20: the audience, and why it is not a `Constraint`

A run can now say how it wants to be told: `offload run --notify push`, a service rather than a
device, defaulting to every route there is. Four things building it settled.

**It is not a `Constraint`, and this ADR's own wording pointed the wrong way.** "Matched by
`Constraint`" was written about *nodes*, which is what a constraint selects — and a phone holding
a push route and a mailbox satisfies `HasService { push }`, after which the obvious implementation
tells that person twice, once by each route. An audience selects **routes**, so it is asked about
one capability at a time, and `Audience::admits` is that question. What the ADR got right is the
part underneath: a route declares a *service* separately from the command that runs it, and
without that separation there would be nothing to name.

**It is named by service and never by route id.** A route id is a node's own name for one of its
sinks — `toast`, on the phone — so naming one would pin a run's news to a device, which is exactly
the thing this plane exists to stop mattering. "Push" is a promise about reaching a person; "the
phone's `toast` script" is a promise about a machine being awake.

**It lives on the spec, and it is not editable.** `RunSpec::notify` travels with the run, for the
reason `queue` does: the node that delivers the news is usually not the node that took the
request, so an audience the submitting process alone remembered would be an audience a migration
silently widened. Wire v10 exists for that direction — a v9 node reads the absent field as
*everyone*, which is a quiet widening rather than a loud failure, and the failure being quiet is
the whole argument for the version bump. It is deliberately **not** a third `SpecEdit`: two
editable fields share one counter on purpose (ADR-0013), and a third arrives with an ADR.

**A silence is chosen or it is explained, at the keyboard.** Two new answers, both while the
person is still there — ADR-0014's argument on this plane. A run that asked for a service no
device in the fleet offers is told so at submission (`no chat:team route in this fleet, so nothing
will tell you`), because the alternative is discovering it at breakfast; and `--notify none` is
said back, for the deadline's reason — it changes what happens and there is no later event to
point at. Neither is a refusal: the work is still worth doing and this project does not cancel a
run over reporting. The other half of keeping it honest is where the filter runs — at the moment
news is *noticed*, not when it is sent, so a route the run never asked for is never owed an outbox
row. And `Audience::Nobody` decides who is interrupted, never what is recorded: the event is in
the log either way.

**Verified on two daemons**, with a fake agent and three routes: push and email on the desktop,
`chat:team` on a phone that hosts nothing. A default run reached all three. `--notify push`
reached the toast alone. `--notify none` reached nobody and said so. `--notify chat:team` was
carried by the *phone* while the desktop's own two routes stayed quiet — three routes in the
fleet, one delivery — and the same flag on a fleet without that route was answered at submission
instead.

## Amendment, 2026-08-21: news that is not about a run

Membership needed carrying (ADR-0012 mitigation 4) and this plane could only address runs. Its
schema said so plainly: the outbox keyed on a run id that was `NOT NULL REFERENCES runs(id)`, and
a sink's cursor was a position in one log.

So a `Notification` carries a **subject** — a run, or the fleet — and the store has a second log
beside the run event log. Two logs rather than one table with a nullable run, because they are two
logs: separate sequences, separate scans, and a fleet event has none of a run event's shape. What
they share is everything downstream, which is the point — one outbox, one cursor table, one retry
policy, one "away is not gone", joined by a `topic` column.

`topic` is half of every key here rather than a label, and the reason is arithmetic: both logs
number from one. A dedup identity of `(seq, sink)` would have the first fleet event mark the first
run's notification as already sent, silently, on any node that ever had both.

Two rules from this ADR needed restating for the new subject and neither changed:

* **A fleet notice ignores a run's audience.** An audience is a field on a run's spec, and
  somebody who scoped their runs' news to their phone has not scoped the alarm that says a device
  joined the fleet. `Audience::admits` is not consulted for the fleet topic at all — which is not
  an exception to "the filter runs where the news is noticed", but an answer to "whose news is
  this": nobody's run.
* **A new sink still starts at the end of both logs.** It matters more here: a route configured
  today must not announce every device that ever joined, because the whole value of this
  particular alarm is being rare enough to read.

The sink payload gained `about` beside `run`, and `run` became nullable rather than being renamed.
That JSON is an interface somebody's script reads, and a rename would break every one of them for
the sake of a tidier field name.

## Alternatives

**Each node sends its own notifications, ad hoc.** Simplest. Rejected: credentials end up
scattered across every device, there is no dedup across a migration, and a run that moves
mid-notification either double-sends or drops.

**A central notification service.** Contradicts ADR-0002 and the personal-fleet premise —
the fleet has no centre by design, and adding one for reporting would make the reporting
path less available than the work it reports on.

**Just poll `offload ps`.** What exists today. It answers nothing when the operator is away
from a terminal, and it cannot answer a prompt at all.

**A separate scheduler for delivery.** Considered and rejected on the strength of the
capability framing: once a sink is a capability, choosing where a notification goes is the
same constraint-match-plus-policy decision as choosing where a run goes. Two schedulers
would mean two sets of refusal reasons for the user to learn, and two places for the
"why didn't that happen" answer to drift.
