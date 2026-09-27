# The delivery plane: sinks, outbox, audience, questions — full entries

The working rules are in `../delivery-and-notifications.md`, in this same order. These are the entries they
were compressed from: the mechanism, the measurement, and how each one was found. Read one
when the rule alone is not enough to act on.

- **An audience selects *routes*, and a watcher needed to select *kinds*.** One axis was doing
  the work of two, and it only showed once ADR-0020's runs started arriving by themselves: every
  trigger walk had run on a node with **no sinks configured**, so the delivery plane was in the
  design and out of the measurement. With one present, a rule firing every three seconds put **11
  notifications on a phone in 40 seconds**, every one `finished after 2 turn(s), $0.0005`. And the
  escape hatch was worse than the problem — `--notify nobody` on the same rule failing every
  firing delivered **0 of 11 failures**, which is the one thing a watcher exists to say. No third
  value existed and none could: `Audience` is asked per capability because it picks routes.
  `RunSpec::notices` is the other axis (ADR-0026), `Problems` is everything except the run
  finishing — a missed deadline and a question are requests for attention too — and `offload
  when` defaults to it while `offload run` does not, which is what a watcher *is*. **Not keyed on
  `Origin`**, which ADR-0024 forbids reaching the delivery plane, and the prohibition is right
  rather than merely binding: some watchers want the heartbeat, and "don't buzz me when it works"
  is a reasonable thing to say about a run you typed. The author decides and it travels.

- **…and then `Problems` threw away the failure it exists for, because it filtered on the log's
  `kind` column.** ADR-0026's own escape hatch, one build later: `Notices::admits` was asked about
  the event log's denormalised kind name, on the stated grounds that the scan filling the outbox
  "never decodes the payload" — and `success` is *in* the payload. An agent reporting `is_error`
  writes `LogKind::Finished { success: false }`, whose kind name is `finished` and which projects
  to `Notice::Failed`; the projection has said so in as many words since it was written ("the same
  news arriving by two routes"). So the two namespaces overlap in three of four kinds and disagree
  about exactly the one this filter turns on. Measured on a rule failing every firing with a
  working sink: **69 firings, 0 notifications**, on a node whose `offload when` had printed "you
  will hear about a failure". `admits` takes the projected `Notice` now, so the wrong string cannot
  be passed; the column still *bounds* the scan, which is all `NOTABLE_KINDS` was ever for. Why it
  shipped: the guard used the other route — the test drove failure through `LogKind::Failed`, which
  is what `fail_if_unfinished` writes when no result event arrives, and the walk used an agent that
  produced the same thing. An agent that runs and *reports* failure was in neither.

- **A question is the one notice that is an *open item*, and the plane treated it like a fact.**
  Two things in one variant, from walking `--ask` and a rule together — both justified by a person,
  on the path where there is none. First: `Notice::NeedsDecision`'s own doc comment says it carries
  "how long there is to give one, because `approve this` with no clock is a promise this plane
  cannot keep", and the variant had three fields and no clock; the patience was in scope at the
  `LogKind::Asked` call site and went to `tracing`, on the machine nobody is logged into, while
  `Notice::Overdue` one arm above renders exactly such a duration. It is not cosmetic: `offload
  approve` addresses the agent's own `tool_use_id`, which exists only while that process is blocked,
  so somebody answering twenty minutes later matches nothing and was never told there was a window.
  Second: `notable` excluded the whole `Answered` kind, correctly for `Allowed` and `Denied`
  ("whoever approved it knows") and on a *fallback* for the third — "what a run did without
  permission is what its **result** reports" — which is a claim about the result being delivered,
  and ADR-0026's `Problems` discards precisely the result. That is a **rule's** default. Measured
  on a rule with a working push route: **2 firings, 2 `asked` notifications, nothing else ever** —
  the phone says a decision is needed, the wait runs out, the agent proceeds, the run completes, and
  the question stops existing in `offload asks` too, so neither end has it. `Notices::Problems`' own
  doc comment had already said "an unanswered question blocks the run mid-turn until its patience
  runs out, which is the last thing to be quiet about"; the question got through and the closing of
  its window did not. `Notice::Undecided` is the close, `answered` joins `NOTABLE_KINDS` as a scan
  bound and not a decision — three outcomes of which one is news, the identical shape to `finished`
  naming two — and `Problems` needed no edit, because `is_good_news` is a predicate over one variant
  rather than a list somebody has to remember to extend.

- **A precondition answered at the keyboard is answered for the command nobody is standing at.**
  Two flags make a promise this fleet may not be able to keep — `--notify <service>` wants a route,
  `--ask` changes what a *blocked run does* — and both notes rode on `Response::Submitted` and
  nothing else. So `offload run --ask --notify push` on a routeless fleet printed two accurate
  warnings and `offload when`, one command later with the same flags, printed **none** and a
  *reassurance* in their place: "you will hear about a failure, a missed deadline or a question",
  three notifications that fleet could deliver none of, and with `--ask` unreachable there would
  never be a question at all. Backwards: a run is answered for by somebody sitting in front of it,
  a rule fires unattended for months. The handler already held the reasoning in a comment above the
  wrong sentence — "a rule is written and then fires unattended for months" — applied to
  `--notify-on` and to neither flag beside it. Three more from the same walk: the **plainest**
  invocation (no flags) was a case, because `audience_note` returns `None` for `Audience::Everyone`
  on the stated grounds that it "needs no words" — true at a keyboard, false for a watcher; and
  `--notify nobody --notify-on everything` claimed `Every firing will be reported` on **any** fleet,
  because ADR-0026's two axes are independent and the sentence about *kinds* never consulted the
  one that zeroes it out. `Reach::{Somebody, NobodyWanted, NoRoute}` is three answers rather than a
  bool for `Removal::{Removed, NothingHere}`'s reason — a silence the author **asked** for must not
  come with advice about adding a route — and it is computed through `Audience::admits` per route so
  a report cannot drift from the plane it describes.

- **…and the third precondition was invisible here because on the other command it is a
  *refusal*.** ADR-0032 fixed `--notify` and `--ask` on `offload when` and left `--use`, whose
  answer at the keyboard is not a note but an error: `offload run --use email` is refused where no
  node offers a mailbox, and `unreachable_resource` lives inside `submit_run`, which is rightly the
  one copy both callers use — so a rule with the same flag is written, fires, and has **every
  occurrence refused**. Measured on a trigger ticking every three seconds: **13 firings, 0 runs in
  thirty seconds**, nothing at the keyboard, nothing on the phone (a refused firing is not a run, so
  there is no run log for the plane to project), and one `WARN` per firing on the machine nobody is
  logged into — under a line reading `Nothing else to do: the next event fires it`. Overnight that
  is 28,800 refusals, and the rule *looks* busy, which the missing-trigger case does not. Warned
  rather than refused (ADR-0036), because a run is submitted now and a rule fires for months into a
  fleet that changes — the mailbox is on the phone, and the phone may not have enrolled yet — which
  is the call the trigger question one line below already makes, in a comment that had written the
  principle down: *"inert-and-silent is how somebody spends a week wondering why their trigger never
  fired"*.

- **An audience selects routes, not nodes** — which is why it is not a `Constraint`, and ADR-0010
  said the opposite before it was built. A constraint chooses a *node*, and a phone holding a push
  route and a mailbox satisfies `HasService { push }`; the obvious implementation then tells that
  person twice, once by each route. `Audience::admits` is therefore asked per capability. And the
  filter runs where news is **noticed** rather than where it is sent: an outbox row is a promise
  to deliver, so a route the run never asked for must never be owed one.

- **A delivery that stopped being retried is not a delivery.** Both are resolved rows in the
  outbox, so `offload sinks` counted them together and reported four successful deliveries for a
  route that had never worked once. Delivered, waiting and dropped are three numbers. A plane
  that cannot explain a silence has failed at its only job; one that reports a healthy number
  while doing so is worse than silent.

- **…and a route that says it cannot deliver is a third thing again.** A peer whose credential
  stops verifying was *filtered out* of the fleet's routes, so the pass found nothing for the rows
  already queued against it and abandoned them — "this route no longer exists in the fleet", said
  about a route `offload sinks` was listing on the next line. `Route::unusable` behaves like
  `reachable`: listed, so the news waits; nothing attempted, so nothing spent. Our *own* broken
  script stays the asymmetric case, attempted and given up on loudly, which is the probe's rule.
  Both facts come from one walk over the view now, because the pass and the report answering "what
  does this peer offer" separately is how they came to disagree.

- **A route that is away is not a route that is gone.** A queued notification for a device the
  fleet still knows waits *indefinitely* — nothing attempted, nothing spent from its retry budget
  — because "you will be told when you pick your phone up" is the promise, and a phone is asleep
  for hours. Only a route the fleet no longer lists at all is given up on. The first version
  conflated them and threw the news away after fifteen seconds. Same distinction as ADR-0007's
  *unreachable is not dead*, on the other plane.

- **…and that makes an away route's rows the oldest in the table, so the queue is per sink.**
  The second-order failure of the rule above: one page across all sinks ordered by sequence was
  all phone once a sleeping device had more undeliverable rows than a page holds, and every other
  route on the node stopped being delivered to — never *selected*, so nothing attempted, nothing
  failed, nothing recorded. `offload sinks` showed a working route with a growing "waiting" count
  and no error beside it, which is the plane causing the silence it exists to explain. The
  `PER_PASS` doc already said "per sink"; the scan half was and the send half was not.

- **A sequence is not a clock, one query further on than you think.** Two logs number
  independently — which is why `topic` is in the dedup key — and the *queue* ordered by sequence
  alone put a fleet event at seq 3 ahead of a run event at seq 900 from a week earlier.
  `noticed_at_ms` is this node's own observation of both logs and is the only ordering that
  matches what the comment claimed.

- **A sink's cursor starts at the end of the log, not the beginning.** Otherwise a route
  configured this evening is handed every notable event the node has ever logged, which is how
  somebody learns to turn notifications off. And the cursor and the outbox row are two mechanisms
  on purpose: advance a cursor only on success and one broken sink swallows everything behind it.

- **Two logs number independently, so a sequence alone is ambiguous.** The run log and the fleet
  log both start at 1, and the outbox is keyed on `(sink, topic, seq)` for that reason: without
  the topic, the first fleet event marks the first run's notification as already sent — silently,
  on any node that has both.

- **A route that gave up on a message has been used, whatever the delivered count says.** Staged
  with four `[[sinks]]` on one node: two that work, one whose script `exit 7`s, and one whose
  program is not there. After a single fleet notice had gone round:

  ```
  SINK         SERVICE    DELIVERED  WAITING  DROPPED  STATE
  flaky        webhook            0        0        1  usable, never used — try `offload sinks --test`
               └─ a route that always refuses
               └─ runs /tmp/ow83/flaky.sh
               └─ last failure: gave up: /tmp/ow83/flaky.sh exited 7: no output
  ```

  `DROPPED 1` in the column, `gave up` in the line underneath, and `never used` in between. The
  match was

  ```rust
  match (&sink.unusable, sink.delivered, sink.waiting) {
      …
      (None, 0, 0) => "usable, never used — try `offload sinks --test`",
      (None, _, waiting) if waiting > 0 => …,
      (None, _, _) if sink.gave_up > 0 => "ok now — {} were given up on earlier",
  ```

  — three counters, two of them matched on, and the third asked about only in a guard *below* the
  arm that swallows it. The `gave_up` arm's own comment says who it is for: *"Usable now and
  something was dropped earlier: worth saying out loud, because the route looks healthy and
  somebody was not told something."* It was unreachable for precisely the route that has told
  nobody anything. Matching on all four fields is the fix; the lesson is the shape — **a guard
  below an arm that already matches is a guard for rows that never arrive.**

  It is a pure function of three numbers and it lived inside the `println!` loop, so no test could
  have failed on it — session eighty-one's rule, met again. `sink_state` is a function now, with a
  test that reproduces the exact string against the old arm ordering.

- **…and the two sink tables ask the same three questions in different orders.** `offload sinks`
  prints this node's routes and then the fleet's, in one function, twenty lines apart. The local
  table asked *delivered anything?* before *gave up on anything?*; the fleet table asked *gave up?*
  before *delivered nothing yet?*. Only the second order is right, and each table reads perfectly
  well on its own — which is why the wrong one survived. Having both on one screen, for routes in
  the same state, is what made it visible.

- **A fleet route's state must not assert a cause the bit does not carry.** Measured on the peer,
  for a `[[sinks]]` entry on alpha naming a program that is not there:

  ```
  alpha/gone   terminal   0  1  0  alpha says its credential does not work
                              └─ last failure: alpha says its credential could not be verified, …
  ```

  and on alpha itself, one command away:

  ```
  gone         terminal   0  0  1  unusable: /tmp/ow83/not-here.sh: not found on this device
  ```

  Two devices, one fact, and the one that has to act on it is told about a credential. The state
  arm is `!route.authenticated`, and `Capability::authenticated` is one bit with a different
  meaning per role (ADR-0019's amendment) and more than one cause within `Role::Sink` — the
  owner's script may be missing, unreadable, or refusing.

  The **measured** reason was already there and was being discarded: since ADR-0019's amendment
  the nominated kinds put label-then-reason in `Capability::description`, `FleetRoute` carries
  that string, and the fleet table printed no description line at all. So the honest sentence was
  travelling across the mesh and being thrown away while the column beside it guessed. The row now
  reads `alpha says it cannot use it` with `└─ a route whose program is missing — its program is
  not on this device` underneath. Still no `runs …` line: ADR-0010's rule is about the *command*.

  `Undeliverable::Unauthenticated`'s `Display` had the same claim one crate down, and
  `offload-core` is the layer that certainly cannot know — it holds the bit and not the machine.
  It says what the bit says.

- **…and a `Display` that will be embedded has to be written for the sentence it lands in.** The
  delivery pass defers an undeliverable row with `format!("{} says {reason}; waiting for it",
  route.node_name)`. The first cut of the reword above read *"the device offering it says it
  cannot use it"*, which came out as

  ```
  last failure: alpha says the device offering it says it cannot use it, so it would drop the
  message; waiting for it
  ```

  Two attributions for one device. Caught by re-running the walk after the fix, not by a test.

- **`offload sinks --test` tries every route, so it must not say it tried the usable ones.** The
  handler is `for report in &mut sinks { … route.deliver(&probe_notice()).await }` — every route,
  with the attempt deciding. The summary said *"A test notification was sent through each usable
  route on this node"*, which claims a filter that does not happen; and `usable` there meant *the
  program resolves*, while the rows above use the same word for *the message went through*. The
  `flaky` route proves the difference: `usable, never used` without `--test`, `unusable:
  /tmp/ow83/flaky.sh exited 7: no output` with it. The summary now says every route was tried and
  what the two outcomes mean.

*The entries below were backfilled in session ninety-one from `docs/sessions.md` and the commits
that added each rule — sourced, not reconstructed from the rule text.*

- **A route's destination may be this node's own rule, and two of the plane's filters then mean
  different things.**

  Session sixty-seven, ADR-0057 (commit `544b21d`). What decided the design was reading the
  delivery tables: **the route key is a `TEXT` id**, so a rule can *be* a route — `rule:<id>` — and
  the cursor, the `(sink, topic, seq)` dedup, the ordering by `noticed_at_ms`, the per-route queue
  and the retries are the ones the plane already has, with no schema change and no wire bump. Every
  one of those was a bug found by measurement in an earlier session; none is obvious enough to get
  right twice.

- **…and the guard that makes it safe is `Origin`, not a counter.**

  Same change. A rule bound to `failed` fires a run; if that run fails it projects the same notice,
  and without a guard the rule fires again for ever. A notice about a run a *machine* started is
  offered to sinks and never to rules — one comparison against a fact that is immutable and
  gossiped (ADR-0024).

- **A report describes a mechanism, so it has to know which one — twice in one change.**

  Same change. With a second way to fire a rule, `trigger_present` became the wrong question:
  `offload when` printed *"nothing on this node watches `failed` — add a [[triggers]] entry"*
  about a rule the plane fires, and `offload rules` printed the same sentence under an escalation
  rule that had just fired. The third instance of one rule in a single session.

- **A notice-bound rule does not see a failure that happened before its first delivery pass.**

  Recorded in commit `ab11a34`'s body, its only source: a notice-bound rule does not see a failure
  in the five seconds before its first delivery pass, because that is when the route's cursor is
  initialised — bounded, honest, and worth knowing before concluding an escalation is broken.

- **An Android channel that must be felt asks for vibration itself, and a changed one gets a new id.**

  Session ninety-two, twice. The phone's first walked question was delivered and opened the right tab,
  but the owner could not say it buzzed. `dumpsys notification` showed the channel at importance 4
  with `mVibrationEnabled=false`. It became `questions-v2`, with a pattern, and the old id was deleted.
  Later the same day a hardware-key approval request on the tablet timed out after 120 s. The
  request had been filed and the notification posted on `approvals`, a high-importance channel with
  the same missing vibration, while the tablet was locked. It became `approvals-v2`. Reading the
  channel on the device is what found both: the code said `IMPORTANCE_HIGH`, and that reads like
  "it will interrupt".

- **A stopped phone hears its news when it is opened.** Session ninety-three built a push "knock"
  for a phone whose daemon had stopped, then removed it the same session. The design was right in
  one respect worth keeping. A scout's map of `deliver.rs` showed that only the run's holder
  notices its news, and that a row for an unreachable route is deferred indefinitely with nothing
  spent. So the push only had to ring the bell (a UnifiedPush POST of "wake"), never carry the news,
  and needed no exactly-once, no dedup, and no trust in the push server. It also backed off per
  node, 5 min doubling to 4 h, because the pass defers every few seconds. The owner declined it at
  the step of installing ntfy on the phone. Every Android push needs a distributor app or FCM, so
  under that constraint a stopped phone is told nothing until it is opened, and the owner accepted
  that ("an ok trade-off"). The whole design is in `git show 5656f34`.
- **"Did not answer" is a device that is away, not a delivery that failed.** Session ninety-four.
  The Mac's log at 10:31:00–10:31:10Z: three `notification not delivered; will retry … reason=phone-app
  did not answer (no route to bbbb2222: no known address — not discovered yet)`, then `giving up on a
  notification; nobody has been told`. The same at 09:54 for the tablet. The Mac had just been
  restarted, and held the phone (whose daemon had stopped under ADR-0079) as alive from gossip, so
  `route.reachable` was true and each send's transport failure was counted. `Cluster::deliver_via`
  now returns `DeliverError::Unanswered` for a request with no answer and `Refused` for an
  `Undelivered` reply; the pass defers on the first. Test
  `a_peer_that_does_not_answer_is_waited_for_and_never_given_up_on`, whose control run with the old
  counting fails "still owed, after 9 passes". The old test
  `a_peer_that_will_not_carry_it_is_retried_and_then_given_up_on` had its fake fail with "the phone
  did not answer", a refusal test written with the words of the other case.
- **…and "not in this node's view" is not "no longer in the fleet".** On the walk the laptop showed
  `emu-fresh/app … DROPPED 1 … last failure: gave up: this route no longer exists in the fleet`: a
  row whose route was missing because the laptop had restarted and not yet re-learned the emulator.
  The pass now asks `Fleet::standing`: `Unheard` waits ("… has not been heard from since this node
  started"), `Revoked` and `Listed` (listed, route withdrawn) give up with their own reasons.
  Walked end to end: the emulator's app backgrounded (its daemon stopped after 101 s), the laptop
  restarted, a queued task went overdue at 15:00:43Z, the tablet got it at once, the emulator's copy
  waited, and it was delivered at 15:01:00.9Z, six seconds after the app was opened. The branch where
  the view says *alive* for a stopped device is covered by the unit test only: staged deliberately,
  the window is a second or two.
