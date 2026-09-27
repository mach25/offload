# The delivery plane: sinks, outbox, audience, questions

Working rules. Full entries — mechanism, measurement, how each was found — in
`detail/delivery-and-notifications.md`, same order.

- **An audience selects *routes*; a watcher needed to select *kinds*.** One axis was doing the work
  of two — `--notify nobody` silenced the failures a watcher exists to report. `RunSpec::notices`
  is the second axis (ADR-0026), `Problems` is everything except the run finishing, and `offload
  when` defaults to it. **Not keyed on `Origin`**: the author decides and it travels.
- **…and `Problems` must filter the projected `Notice`, not the log's `kind` column.** `success` is
  in the payload, so `LogKind::Finished { success: false }` has kind name `finished` and projects
  to `Notice::Failed` — the two namespaces overlap in three of four kinds and disagree about
  exactly the one this turns on. The column still *bounds* the scan; it never decides.
- **A question is an open item, not a fact.** `Notice::NeedsDecision` carries the patience, because
  `offload approve` addresses a `tool_use_id` that exists only while the process is blocked.
  `Notice::Undecided` closes the window; `answered` is a scan bound, not a decision.
- **A precondition answered at the keyboard is answered for the command nobody is standing at.**
  `offload when` fires unattended for months, so it needs the warnings `offload run` prints — and
  the *plainest* invocation is a case, because `Audience::Everyone` "needs no words" only at a
  keyboard. `Reach::{Somebody, NobodyWanted, NoRoute}`, computed through `Audience::admits` so a
  report cannot drift from the plane it describes.
- **…and the third precondition is a *refusal* on the neighbouring command.** `--use` with no such
  resource is refused on `offload run`, so a rule with it had every occurrence refused, silently.
  Warned rather than refused (ADR-0036): a rule fires for months into a fleet that changes.
- **An audience selects routes, not nodes**, which is why it is not a `Constraint` — a phone
  holding push *and* a mailbox would satisfy the constraint and be told twice. `Audience::admits`
  is asked per capability, and the filter runs where news is **noticed**, not where it is sent: an
  outbox row is a promise, so a route the run never asked for must never be owed one.
- **A delivery that stopped being retried is not a delivery.** Delivered, waiting and dropped are
  three numbers. A plane that cannot explain a silence has failed at its only job.
- **…and a route that says it cannot deliver is a third thing again.** `Route::unusable` is listed
  (so news waits) and never attempted (so nothing is spent). Our *own* broken script stays the
  asymmetric case. One walk over the view feeds both the pass and the report.
- **A route that is away is not a route that is gone.** A queued notification for a device the
  fleet still knows waits *indefinitely*. Only a route the fleet no longer lists is given up on.
- **…so the queue is per sink.** One page ordered by sequence across all sinks is all phone once a
  sleeping device has more undeliverable rows than a page holds — every other route silently
  starved, never selected, nothing recorded.
- **A sequence is not a clock.** Two logs number independently, so ordering the queue by sequence
  put a fleet event at seq 3 ahead of a run event at seq 900 from a week earlier. Order by
  `noticed_at_ms`, this node's own observation of both logs.
- **A sink's cursor starts at the end of the log, not the beginning**, or a route configured this
  evening is handed everything the node ever logged. Cursor and outbox row are two mechanisms:
  advance a cursor only on success, or one broken sink swallows everything behind it.
- **Two logs number independently, so a sequence alone is ambiguous.** The outbox is keyed on
  `(sink, topic, seq)`; without the topic the first fleet event marks the first run's notification
  as already sent.
- **A route's destination may be this node's own rule, and two of the plane's filters then mean
  different things.** ADR-0057 makes an escalation a *delivery*: a notice-bound rule is a route
  keyed `rule:<id>`, which is free because the route key was already a `TEXT` id — cursor, dedup,
  ordering, per-route queue and retries all apply with no schema change. What does **not** apply
  is `Audience`, and the reason is ADR-0026's: it selects routes *to people*, so reading
  `--notify nobody` as *do not recover this run* would be one axis doing the work of two. The
  precedent was already in the pass — the fleet's own log is offered to every route with no
  audience asked. `Notices` **is** asked of both, and costs nothing, because `Problems` and
  `Everything` both admit a failure.
- **…and the guard that makes it safe is `Origin`, not a counter.** A rule bound to `failed`
  fires a run; if that run fails it projects the same notice, and without a guard the rule fires
  again for ever, one agent at a time, all night. A notice about a run whose origin is `Rule` is
  offered to sinks and **never to rules** — one comparison against a fact that is immutable and
  gossiped, so no node has to be asked. What it costs is that a chain of two escalations is
  unsayable, which is stated in the ADR rather than discovered.
- **A report describes a mechanism, so it has to know which one — twice in one change.** With a
  second way to fire a rule, `trigger_present` became the wrong question: `offload when` printed
  *"nothing on this node watches `failed` — add a [[triggers]] entry"* about a rule the delivery
  plane fires, and `offload rules` printed the same sentence under an escalation rule that **had
  just fired**. Both now carry `fired_by` and say the sentence that is true of their own
  mechanism. Third instance of this rule in one session; the way to find them is to run the thing
  and read the output.
- **A notice-bound rule does not see a failure that happened before its first delivery pass.** A
  route's cursor starts at the *end* of the log — right, and the entry above says why — and it is
  initialised when the pass first sees the route, not when the rule was written. Measured: a rule
  created and a run failed within the same five-second interval, and the escalation did not fire;
  the next failure fired it. Bounded by the pass's own cadence and honest, but worth knowing
  before concluding an escalation is broken.
- **A route that gave up on a message has been used, whatever the delivered count says.** `offload
  sinks` matched two of a route's three counters — `(unusable, delivered, waiting)` — and left
  `gave_up` to a guard below, so a sink that had delivered nothing, had nothing queued and had
  **abandoned a notification** came out `usable, never used — try offload sinks --test`, with
  `DROPPED 1` in the column beside it and its own `last failure: gave up: …` line printed directly
  underneath. The arm that says what that means was written for exactly this — *"the route looks
  healthy and somebody was not told something"* — and was shadowed by the one above it, in the case
  where the route looks least healthy of all. **When a match is on a subset of the fields a guard
  below asks about, the guard is unreachable for the rows it was written for.**
- **…and the two sink tables ask the same three questions in different orders, twenty lines apart.**
  The local one asked *has it delivered anything* before *has it given up on anything*; the fleet
  one asked them the other way round and was right. One function, one screen, one idea rendered
  twice — which is how the wrong order survived, because each table looks correct on its own.
- **A fleet route's state must not assert a cause the bit does not carry.** `!authenticated` was
  rendered *"alpha says its credential does not work"*, for a `[[sinks]]` entry whose program is
  simply not on that device — while alpha's own table one command away said `not found on this
  device`. `Capability::authenticated` is one bit with a different meaning per role and more than
  one cause within a role. It says `says it cannot use it` now, and the **measured** reason has
  been travelling in `Capability::description` since ADR-0019's amendment and was being thrown
  away: the fleet listing prints it. Same for `Undeliverable::Unauthenticated`, which is
  `offload-core` and holds the bit and not the machine.
- **…and a `Display` that will be embedded has to be written for the sentence it lands in.** The
  delivery pass wraps that reason as `{node} says {reason}; waiting for it`, so the first cut of
  the fix — *"the device offering it says it cannot use it"* — produced *"alpha says the device
  offering it says it cannot use it"*. Write the variant as a predicate of whatever the wrapper
  names.
- **`offload sinks --test` tries every route, so it must not say it tried the usable ones.** The
  handler loops over all of them and lets the attempt decide — which is the whole point, since
  `usable` before a test means only *the program resolves*, and the test is what tells a route
  that resolves and then refuses from one that works. The summary claimed a filter that does not
  happen, and used `usable` for a different question from the one the rows above answer with the
  same word.
- **An Android channel that must be felt asks for vibration itself, and a changed one gets a new
  id.** `IMPORTANCE_HIGH` alone does not vibrate (`mVibrationEnabled=false` in `dumpsys
  notification`), and a channel's settings are fixed once it exists, so editing the code changes
  nothing on an installed device. Both the Questions and the Approvals channel shipped without it,
  and an approval request expired unnoticed on a locked tablet. Read the channel on the device, not
  the builder call.
- **A stopped phone hears its news when it is opened, and that is by the owner's choice.** News for
  an away route waits on its holder (the rule above), so a phone whose daemon has stopped (ADR-0079)
  loses nothing, only hears late. A push would need a distributor app or Google's FCM, and the owner
  declined a third-party app on the phone. The knock that was built for it is in ADR-0079's
  withdrawn section. Do not re-propose push without a way that needs neither.
- **"Did not answer" is a device that is away, not a delivery that failed.** `deliver_via` folded
  *no answer* (no address, a timeout) and *the peer's route failed* into one `String`, and the pass
  spent one of three attempts on each. A node that had just restarted held the stopped phone as alive,
  every send found no address, and ten seconds later the news was abandoned: ADR-0079's "a stopped
  phone only hears late" was false. `DeliverError::{Unanswered, Refused}`, and only `Refused` counts.
  The fake fleet had failed its *refusal* test with "did not answer", which is how the two stayed one.
- **…and "not in this node's view" is not "no longer in the fleet".** A restarted node knows almost
  nobody until gossip refills its view, so every queued row for an away device read as a route that
  no longer exists. Gone means revoked, or listed and no longer offering the route (`Standing`);
  unheard waits. Both walked on the emulator, and both control runs fail.
